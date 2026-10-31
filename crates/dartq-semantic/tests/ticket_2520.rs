//! Pin: extension specificity orders by the instantiated extended type's
//! parameter-level subtype (#2520).
//!
//! `_isMoreSpecific` compares the extension's INSTANTIATED extended type
//! (upstream Dart SDK source,
//! pkg/analyzer/lib/src/dart/resolver/extension_member_resolver.dart:484-497
//! — the receiver-inferred form `applicableTo` builds in
//! resolver/applicable_extensions.dart) before it falls to the
//! instantiate-to-bounds projections (:500-506).
//!
//! Steps 3-4 read the `on` types at the name level, where a parameter leaf
//! can only be matched by spelling: `on Bag<List<T>>` with `T extends
//! Fragment` and `on Bag<List<Fragment>>` stay mutually compatible, and
//! step 5's instantiate-to-bounds projections are the same type
//! (`Bag<List<Fragment>>` both), so the pair never ordered and the access
//! reported `ambiguous_extension_member_access`. The step-5 comparison now
//! reads each face's own parameters at their declared bounds against the
//! twin's written `on` type first — `Bag<List<Fragment>>` against
//! `Bag<List<T>>` orders one way only — which recovers the order the
//! instantiated extended types would give (`Bag<List<SubFragment>>` is a
//! strict subtype of `Bag<List<Fragment>>`).
//!
//! The twin faces differ in member arity so the chosen face is observable
//! from diagnostics alone: the generic face's `act()` takes no positional
//! argument, the plain face's `act(int)` requires one.
//!
//! Not covered here: the shape whose `on` arguments are a bare parameter
//! against a concrete spelling (`on Bag<T>` vs `on Bag<Fragment>`). That
//! pair is declined by the steps 3-4 gate before the structural read runs,
//! so ordering it needs the receiver-instantiated comparison at that call
//! site — see `try_bounds_subtype`'s seam note.

use std::path::Path;

use dartq_language::{Feature, FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_resolution::{
    ClassModifiers, Combinators, ImportEdge, LibraryClassModifiers, LibraryGraphBuilder,
    Namespaces,
};
use dartq_scanner::{ScanConfig, scan};
use dartq_semantic::FileReport;
use dartq_semantic::capture_clause_trees;
use dartq_semantic::collector::collect;
use dartq_semantic::diagnose::{StrictModes, fold_unit_with_ops};
use dartq_semantic::library::ElementIndex;
use dartq_semantic::model::MicroKind;
use dartq_semantic::resolved_ops::{ClauseTrees, LibraryTypes, ResolvedTypeOps};
use dartq_source::{FileTable, StringTable};

/// The synthetic dart:core names these fragments consume (the engine's
/// implicit core import resolves the same spellings).
const CORE_FACE: &[&str] = &["Object", "bool", "double", "int", "String"];

/// The pinned shape: a `Bag<List<SubFragment>>` receiver, a concrete `on
/// Bag<List<Fragment>>` face and a generic `on Bag<List<T>>` face whose
/// parameter is bounded by the concrete twin's argument.
const SUBTYPE_RECEIVER: &str = "\
class Fragment {}
class SubFragment extends Fragment {}
class List<T> {}
class Bag<T> {}
extension Plain on Bag<List<Fragment>> {
  void act(int x) {}
}
extension Gen<T extends Fragment> on Bag<List<T>> {
  void act() {}
}
void use(Bag<List<SubFragment>> b) {
  b.act();
}
";

/// Two concrete faces on the same `on` type: equally specific at every
/// step, so the access stays ambiguous — the ordering arm only fires on a
/// one-way order, never on a mutual one.
const CONCRETE_TWIN: &str = "\
class Fragment {}
class Bag<T> {}
extension One on Bag<Fragment> {
  void act(int x) {}
}
extension Two on Bag<Fragment> {
  void act() {}
}
void use(Bag<Fragment> b) {
  b.act();
}
";

/// Runs `source` through the live pipeline with the resolved-type
/// snapshot mounted the way the engine mounts it (the clause-tree capture
/// feeds the containers' written supertype trees and parameter bounds into
/// `LibraryTypes`), so the strict applicability and specificity arms
/// answer.
fn unit_report(source: &str) -> FileReport {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
    let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
    let features = ParserFeatures::from_feature_set(set);
    let scanned = scan(
        source,
        ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: set.is_enabled(Feature::Augmentations),
        },
        file,
    );
    assert!(
        scanned.diagnostics.is_empty(),
        "scanner diagnostics: {scanned:?}"
    );
    let elements = collect(source, &scanned.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib = builder.intern("package:t/main.dart");
    let core = builder.intern("dart:core");
    let core_file = files.intern(Path::new("<dart:core>"));
    for name in CORE_FACE {
        builder.declare(core, strings.intern(name), core_file);
    }
    builder.import(
        lib,
        ImportEdge {
            target: Some(core),
            prefix: None,
            deferred: false,
            combinators: Combinators::unfiltered(),
        },
    );
    let mut index = ElementIndex::default();
    index.insert(file, elements.clone());
    builder.unit(lib, file);
    for (name, decl) in elements.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        if matches!(
            decl.kind,
            MicroKind::Class | MicroKind::Enum | MicroKind::Mixin
        ) {
            builder.declare_class(lib, name, file, ClassModifiers::default());
        } else {
            builder.declare(lib, name, file);
        }
    }
    let mut clauses = ClauseTrees::empty();
    capture_clause_trees(
        file,
        source,
        &scanned.tokens,
        features,
        &strings,
        &mut clauses,
    );
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    let types =
        LibraryTypes::build_with_clauses(&graph, &namespaces, &index, &strings, lib, &clauses);
    let ops = ResolvedTypeOps::new(&types, &strings, file);
    fold_unit_with_ops(
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
        set.is_enabled(Feature::InferenceUpdate2),
        Some(&ops),
    )
    .report
}

/// The report's diagnostic code names in source order.
fn codes(report: &FileReport) -> Vec<&'static str> {
    report.diagnostics.iter().map(|d| d.code.name).collect()
}

/// The pinned evidence: the instantiated `on` types order (`Bag<List<
/// SubFragment>>` strictly below `Bag<List<Fragment>>`), so the generic
/// member resolves — no `ambiguous_extension_member_access`, and the
/// resolved member is the no-argument `act()` (the plain face's `act(int)`
/// would report the positional deficit).
#[test]
fn instantiated_on_type_order_picks_the_generic_face() {
    let report = unit_report(SUBTYPE_RECEIVER);
    let got = codes(&report);
    assert!(
        !got.contains(&"ambiguous_extension_member_access"),
        "the generic face must dominate by the instantiated on-type order, got {got:?}"
    );
    assert!(
        !got.contains(&"not_enough_positional_arguments"),
        "the generic face's zero-argument act() must be the resolved member, got {got:?}"
    );
}

/// Two concrete faces on the same `on` type are equally specific and the
/// access stays ambiguous.
#[test]
fn concrete_twin_faces_stay_ambiguous() {
    let report = unit_report(CONCRETE_TWIN);
    let got = codes(&report);
    assert!(
        got.contains(&"ambiguous_extension_member_access"),
        "two equally specific concrete faces must stay ambiguous, got {got:?}"
    );
}
