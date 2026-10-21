//! The #2431 pins: the extension face's type-parameter bounds — the two
//! oracle behaviors the face model now carries (`type_param_bounds`).
//!
//! The applicability gate: an `on` argument naming an extension's own type
//! parameter instantiates against the receiver's argument, then the bound
//! decides — an argument the bound rejects fails the inference and the
//! face drops out of the pool (`applicable_extensions.dart` `applicableTo`
//! through `GenericInferrer.tryChooseFinalTypes`; upstream Dart SDK
//! source, pkg/analyzer/lib/src/dart/resolver/applicable_extensions.dart
//! and pkg/analyzer/lib/src/dart/element/generic_inferrer.dart). The
//! informative_data.dart race (`_ListOfPropertyAccessorFragment<T extends
//! PropertyAccessorFragmentImpl> on List<T>` vs its siblings) resolves
//! through exactly the face whose bound accepts the receiver.
//!
//! The specificity tail: mutually compatible `on` types order on the
//! instantiate-to-bounds projections (`extension_member_resolver.dart`
//! `_isMoreSpecific` step 5 through `_instantiateToBounds`; upstream Dart
//! SDK source, pkg/analyzer/lib/src/dart/resolver/
//! extension_member_resolver.dart:459-506) — the collection package's
//! `IterableComparableExtension<T extends Comparable<T>>` is the more
//! specific face against `IterableExtension<T>`, and a bound-less
//! parameter projects to `dynamic` (the Dart reading of unbounded), which
//! only the tops accept.
//!
//! Oracle provenance: each expectation was probed with `dart analyze`
//! (Dart 3.13.1 via fvm 3.47.1) over the /tmp/probe2431 fixtures during
//! this ticket — lib/a.dart (bounded-over-unbounded domination, zero
//! issues), lib/b.dart (bound-rejecting face drops: :15/:20/:27 zero
//! issues, the raw-`List` receiver answers `undefined_getter` on
//! `List<dynamic>` at :38), lib/c.dart (the informative_data three-face
//! shape, zero issues), lib/d.dart (`sorted()` resolves through the
//! bounded face at :17; a non-Comparable receiver drops it and the
//! remaining face's required argument reports `not_enough_positional_
//! arguments` at :26).

use super::*;
use crate::collector::collect;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_resolution::LibraryGraphBuilder;
use dartq_scanner::ScanConfig;
use dartq_source::FileTable;
use std::path::Path;

/// Diagnoses `source` as the sole file of a library importing nothing but
/// the seeded dart:core stub — the `ticket_2423_tests` rig mirrored, with
/// `Iterable` seeded beside the collection shapes the bounds faces sit on.
fn report(source: &str) -> FileReport {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
    let core_file = files.intern(Path::new("<dart:core>"));
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
    let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
    let features = ParserFeatures::from_feature_set(set);
    let elements = collect(source, &scanned.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib = builder.intern("package:t/main.dart");
    let mut index = ElementIndex::default();
    index.insert(file, elements.clone());
    for (name, decl) in elements.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(lib, name, file);
    }
    builder.seed_implicit_core(
        lib,
        core_file,
        &[
            "Object",
            "String",
            "int",
            "bool",
            "double",
            "num",
            "List",
            "Map",
            "void",
            "Iterable",
            "Comparable",
            "Set",
        ],
    );
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    let fold = fold_unit(
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
        set.is_enabled(dartq_language::Feature::InferenceUpdate2),
    );
    let mut report = fold.report;
    report.diagnostics = merge_source_ordered(fold.parse_diagnostics, report.diagnostics);
    report
}

fn code_names(report: &FileReport) -> Vec<&'static str> {
    report
        .diagnostics
        .iter()
        .map(|d| d.code.name)
        .collect::<Vec<_>>()
}

/// The diagnostics' code-plus-message view — the row identities a failing
/// assert needs (which local, which face).
fn code_views(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .map(|d| format!("{}: {}", d.code.name, d.message))
        .collect()
}

/// Step 5's instantiate-to-bounds domination: the bounded face is the more
/// specific one against its unbounded twin — the collection package's
/// `IterableComparableExtension<T extends Comparable<T>>` shape (the
/// /tmp/probe2431/lib/a.dart verdicts: both receiver shapes resolve, zero
/// issues).
#[test]
fn bounded_face_dominates_its_unbounded_twin() {
    let source = "class MyComparable implements Comparable<MyComparable> {\n\
                  \x20 int compareTo(MyComparable other) => 0;\n\
                  }\n\
                  extension BoundedExt<T extends Comparable<T>> on Iterable<T> {\n\
                  \x20 List<T> get sorted => const [];\n\
                  }\n\
                  extension PlainExt<T> on Iterable<T> {\n\
                  \x20 List<T> get sorted => const [];\n\
                  }\n\
                  void useStrings(List<String> xs) {\n\
                  \x20 xs.sorted;\n\
                  }\n\
                  void useMyComparable(List<MyComparable> xs) {\n\
                  \x20 xs.sorted;\n\
                  }\n";
    assert!(
        code_names(&report(source)).is_empty(),
        "{:?}",
        code_views(&report(source))
    );
}

/// The over-domination guard: two faces bounded by the SAME type project
/// to identical instantiate-to-bounds types — mutually unordered, the
/// access stays ambiguous (step 5 orders only a strict projection
/// subtype).
#[test]
fn equally_bounded_twin_faces_stay_ambiguous() {
    let source = "class Num {}\n\
                  extension B1<T extends Num> on List<T> {\n\
                  \x20 int get probe => 1;\n\
                  }\n\
                  extension B2<T extends Num> on List<T> {\n\
                  \x20 int get probe => 2;\n\
                  }\n\
                  int use(List<Num> xs) => xs.probe;\n";
    assert_eq!(
        code_names(&report(source)),
        ["ambiguous_extension_member_access"],
        "{:?}",
        code_views(&report(source))
    );
}

/// The applicability gate: two bounded faces racing on one member — only
/// the face whose bound accepts the receiver's argument applies, and the
/// mirrored site resolves through the other face (the /tmp/probe2431/
/// lib/b.dart :15/:20 verdicts, zero issues).
#[test]
fn bound_rejecting_face_drops_out_of_the_pool() {
    let source = "class Fragment {}\n\
                  class AccessorFragment extends Fragment {}\n\
                  class InducingFragment extends Fragment {}\n\
                  extension AccessorExt<T extends AccessorFragment> on List<T> {\n\
                  \x20 Iterable<T> get withOrigin => const [];\n\
                  }\n\
                  extension InducingExt<T extends InducingFragment> on List<T> {\n\
                  \x20 Iterable<T> get withOrigin => const [];\n\
                  }\n\
                  void useAccessors(List<AccessorFragment> xs) {\n\
                  \x20 xs.withOrigin;\n\
                  }\n\
                  void useInducing(List<InducingFragment> xs) {\n\
                  \x20 xs.withOrigin;\n\
                  }\n";
    assert!(
        code_names(&report(source)).is_empty(),
        "{:?}",
        code_views(&report(source))
    );
}

/// A subtype argument satisfies the bound through its declared edge: the
/// face applies to `List<SubAccessor>` under the
/// `T extends AccessorFragment` bound (the /tmp/probe2431/lib/b.dart :27
/// verdict, zero issues).
#[test]
fn subtype_argument_satisfies_the_bound() {
    let source = "class Fragment {}\n\
                  class AccessorFragment extends Fragment {}\n\
                  class SubAccessor extends AccessorFragment {}\n\
                  extension AccessorExt<T extends AccessorFragment> on List<T> {\n\
                  \x20 Iterable<T> get withOrigin => const [];\n\
                  }\n\
                  void useSubAccessors(List<SubAccessor> xs) {\n\
                  \x20 xs.withOrigin;\n\
                  }\n";
    assert!(
        code_names(&report(source)).is_empty(),
        "{:?}",
        code_views(&report(source))
    );
}

/// A raw `List` receiver instantiates its argument to `dynamic`, and
/// `dynamic` fails every concrete bound — both faces drop, no extension
/// applies, and the access keeps its own `undefined_getter` (the
/// /tmp/probe2431/lib/b.dart :38 verdict, verbatim).
#[test]
fn raw_receiver_fails_every_concrete_bound() {
    let source = "class Fragment {}\n\
                  class AccessorFragment extends Fragment {}\n\
                  class InducingFragment extends Fragment {}\n\
                  extension AccessorExt<T extends AccessorFragment> on List<T> {\n\
                  \x20 Iterable<T> get withOrigin => const [];\n\
                  }\n\
                  extension InducingExt<T extends InducingFragment> on List<T> {\n\
                  \x20 Iterable<T> get withOrigin => const [];\n\
                  }\n\
                  void useAccessors(List<AccessorFragment> xs) {\n\
                  \x20 xs.withOrigin;\n\
                  }\n\
                  void useInducing(List<InducingFragment> xs) {\n\
                  \x20 xs.withOrigin;\n\
                  }\n\
                  void useRaw(List xs) {\n\
                  \x20 xs.withOrigin;\n\
                  }\n";
    assert_eq!(
        code_names(&report(source)),
        ["undefined_getter"],
        "{:?}",
        code_views(&report(source))
    );
}

/// The informative_data.dart shape: a concrete unbounded face beside two
/// bounded siblings, all declaring the same member — each site resolves
/// through its own face (the concrete one covariantly, the bounded ones
/// through their bounds; the /tmp/probe2431/lib/c.dart verdicts, zero
/// issues).
#[test]
fn concrete_sibling_resolves_beside_a_bounded_pair() {
    let source = "class ConstructorFragment {}\n\
                  class PropertyAccessorFragment {}\n\
                  class PropertyInducingFragment {}\n\
                  extension on List<ConstructorFragment> {\n\
                  \x20 Iterable<ConstructorFragment> get withOriginDeclaration =>\n\
                  \x20\x20\x20\x20 const <ConstructorFragment>[];\n\
                  }\n\
                  extension _ListOfPropertyAccessorFragment<\n\
                  \x20 T extends PropertyAccessorFragment\n\
                  > on List<T> {\n\
                  \x20 Iterable<T> get withOriginDeclaration => const [];\n\
                  }\n\
                  extension _ListOfPropertyInducingFragment<\n\
                  \x20 T extends PropertyInducingFragment\n\
                  > on List<T> {\n\
                  \x20 Iterable<T> get withOriginDeclaration => const [];\n\
                  }\n\
                  void useAccessors(List<PropertyAccessorFragment> xs) {\n\
                  \x20 xs.withOriginDeclaration;\n\
                  }\n\
                  void useConstructors(List<ConstructorFragment> xs) {\n\
                  \x20 xs.withOriginDeclaration;\n\
                  }\n\
                  void useInducing(List<PropertyInducingFragment> xs) {\n\
                  \x20 xs.withOriginDeclaration;\n\
                  }\n";
    assert!(
        code_names(&report(source)).is_empty(),
        "{:?}",
        code_views(&report(source))
    );
}

/// A bound failure falls through: the bounded `sorted` drops for a
/// receiver whose argument provably fails the bare-head bound and the
/// access resolves through the remaining plain face — the dropped face's
/// member and its optional parameter report unreferenced (the
/// /tmp/probe2431/lib/d3.dart oracle verdicts on the same fixture:
/// `unused_element` + `unused_element_parameter` on the dropped face,
/// plus its `not_enough_positional_arguments` on the plain face's
/// required argument — that arity face through an applied extension
/// member is outside this lane's scope and stays unpinned).
#[test]
fn bound_failure_falls_through_to_the_remaining_face() {
    let source = "class Num {}\n\
                  class Rule {}\n\
                  extension _BoundedSorted<T extends Num> on Iterable<T> {\n\
                  \x20 List<T> sorted([int Function(T a, T b)? compare]) => const [];\n\
                  }\n\
                  extension _PlainSorted<T> on Iterable<T> {\n\
                  \x20 List<T> sorted(int Function(T a, T b) compare) => const [];\n\
                  }\n\
                  void useRules(Set<Rule> rules) {\n\
                  \x20 rules.sorted();\n\
                  }\n";
    // Source order (the oracle's own row order on the same fixture:
    // UNUSED_ELEMENT at the dropped member's name, col 11, before
    // UNUSED_ELEMENT_PARAMETER at `compare`, col 43).
    assert_eq!(
        code_names(&report(source)),
        ["unused_element", "unused_element_parameter"],
        "{:?}",
        code_views(&report(source))
    );
}
