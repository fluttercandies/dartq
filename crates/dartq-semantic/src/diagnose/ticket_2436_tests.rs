//! The #2436 pins: the concrete-implementation completeness loop
//! (`implementation_overrides`) skips the signature comparison when either
//! compared side carries its declaring container's free type parameters —
//! the unsubstituted read the element model hands the loop (clause type
//! arguments stripped to bare names) false-breaks on the free heads (the
//! covariant result face's Left-Nullable arm, the dead-edge arm).
//!
//! Mechanism (the #2435 audit, `/tmp/probe2435`): `face_entries` collects
//! interface members raw, so `class ReplacementVisitor implements
//! TypeVisitor<TypeImpl?>` left the interface's `R visitDynamicType(...)`
//! spelling unsubstituted and every subclass inheriting the concrete
//! members emitted `invalid_implementation_override` against it — 74
//! dartq-only rows on the analyzer corpus, all silent in the oracle. The
//! substitution environment is undecidable in this loop by design (it
//! tracks no edge chain; a foreign hop's clauses live in another file's
//! tokens), so the comparison skips — the under-report posture every other
//! arm keeps. The single-member `check_override` path keeps its
//! substitution face (exact same-file edges) and is untouched.
//!
//! Oracle provenance: the three fixture shapes below are the #2435 audit's
//! probes (`/tmp/probe2435/lib/iio_generic_iface.dart`,
//! `iio_nodelist.dart`, `iio_samefile.dart`) — all silent under `dart
//! analyze` (Dart 3.13.1, fvm 3.47.1); dartq emitted the
//! `invalid_implementation_override` row on the generic-interface shape
//! pre-fix.

use super::*;
use crate::collector::collect;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_resolution::{Combinators, ImportEdge, LibraryGraphBuilder, Namespaces};
use dartq_scanner::ScanConfig;
use dartq_source::FileTable;
use std::path::Path;

/// Single-file rig: `main.dart` with the implicit `dart:core` seed the
/// fixtures' `int` spellings resolve through.
fn report(source: &str) -> FileReport {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file_main = files.intern(Path::new("main.dart"));
    let core_file = files.intern(Path::new("<dart:core>"));
    let scanned = dartq_scanner::scan(
        source,
        ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: true,
        },
        file_main,
    );
    assert!(
        scanned.diagnostics.is_empty(),
        "scanner diagnostics: {source:?}"
    );
    let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
    let features = ParserFeatures::from_feature_set(set);
    let elements = collect(source, &scanned.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib_main = builder.intern("package:t/main.dart");
    let mut index = ElementIndex::default();
    index.insert(file_main, elements.clone());
    for (name, decl) in elements.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(lib_main, name, file_main);
    }
    builder.seed_implicit_core(
        lib_main,
        core_file,
        &[
            "Object", "String", "int", "bool", "double", "num", "List", "Map", "Iterable", "void",
        ],
    );
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    let fold = fold_unit(
        source,
        &scanned.tokens,
        features,
        &strings,
        file_main,
        lib_main,
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

fn has(names: &[&'static str], code: &str) -> bool {
    names.contains(&code)
}

/// The override family the gate owns — a clean run names none of them.
fn override_family_clean(names: &[&'static str]) -> bool {
    !has(names, "invalid_implementation_override")
        && !has(names, "invalid_implementation_override_setter")
        && !has(names, "invalid_override")
        && !has(names, "invalid_override_setter")
}

/// Pin (a), the generic-interface shape (`iio_generic_iface.dart`): the
/// interface's members spell the container's parameter (`R visit...`), the
/// implementor substitutes (`implements TypeVisitor<TypeImpl?>`), and the
/// subclass inherits the concrete members — the oracle is silent on the
/// whole file, and the unsubstituted read's Left-Nullable break
/// (`TypeImpl?` against raw `R`) must not resurface as
/// `invalid_implementation_override` at the subclass.
#[test]
fn generic_interface_substituted_members_stay_silent() {
    let source = "abstract class TypeVisitor<R> {\n\
                  \x20 R visitDynamicType(DynamicType t);\n\
                  \x20 R visitVoidType(VoidType t);\n\
                  }\n\
                  class DynamicType {}\n\
                  class VoidType {}\n\
                  class TypeImpl {}\n\
                  class InterfaceTypeImpl extends TypeImpl {}\n\
                  class ReplacementVisitor implements TypeVisitor<TypeImpl?> {\n\
                  \x20 const ReplacementVisitor();\n\
                  \x20 TypeImpl? visitDynamicType(DynamicType t) => null;\n\
                  \x20 TypeImpl? visitVoidType(VoidType t) => null;\n\
                  }\n\
                  class LeastGreatestClosureHelper extends ReplacementVisitor {\n\
                  \x20 const LeastGreatestClosureHelper();\n\
                  }\n";
    let names = code_names(&report(source));
    assert!(
        override_family_clean(&names),
        "the substituted interface members never report: {names:?}"
    );
}

/// Pin (b), the over-suppression guard: the same inherited-concrete
/// architecture with CONCRETE types keeps its row — `Host`'s `Base?`
/// return is not a subtype of the interface's `Base`, so the subclass
/// reports `invalid_implementation_override` (and the own member reports
/// the per-member `invalid_override`), exactly as the oracle does. The
/// gate only fires on free type parameters, never on a genuine mismatch.
#[test]
fn concrete_type_mismatch_still_reports() {
    let source = "class Base {}\n\
                  abstract class Iface {\n\
                  \x20 Base member();\n\
                  }\n\
                  class Host implements Iface {\n\
                  \x20 Base? member() => null;\n\
                  }\n\
                  class Sub extends Host {}\n";
    let names = code_names(&report(source));
    assert!(
        has(&names, "invalid_override"),
        "the own nullable-return member still reports: {names:?}"
    );
    assert!(
        has(&names, "invalid_implementation_override"),
        "the subclass's inherited concrete member still reports against the interface: {names:?}"
    );
}

/// Pin (c), the operator-setter shape (`iio_nodelist.dart`): a generic
/// container's `operator []=` carries `E`, the application's concrete
/// override spells the argument type, and the `[]=` spelling rides the
/// setter face — the free-parameter gate suppresses the inherited-entry
/// comparison the oracle never runs unsubstituted. Silent, like the
/// oracle.
#[test]
fn operator_setter_generic_container_stays_silent() {
    let source = "class NodeListImpl<E> {\n\
                  \x20 E operator [](int index) => throw 0;\n\
                  \x20 void operator []=(int index, E node) {}\n\
                  }\n\
                  class FormalParameterImpl {}\n\
                  final class FormalParameterListV1NodeList\n\
                  \x20   extends NodeListImpl<FormalParameterImpl> {\n\
                  \x20 @override\n\
                  \x20 FormalParameterImpl operator [](int index) =>\n\
                  \x20     super[index];\n\
                  \x20 @override\n\
                  \x20 void operator []=(int index, FormalParameterImpl node) {\n\
                  \x20   super[index] = node;\n\
                  \x20 }\n\
                  }\n";
    let names = code_names(&report(source));
    assert!(
        override_family_clean(&names),
        "the operator faces stay silent: {names:?}"
    );
}

/// Pin (d), the same-file generic edge (`iio_samefile.dart`): the
/// substitution is decidable here and the concrete member matches it —
/// the whole file stays silent, and the gate must not introduce a row the
/// raw file never had.
#[test]
fn same_file_generic_edge_stays_silent() {
    let source = "abstract class InferenceTypeVisitor<R> {\n\
                  \x20 R visitUnknownInferredType(UnknownInferredType t);\n\
                  }\n\
                  class UnknownInferredType {}\n\
                  class TypeImpl2 {}\n\
                  abstract class TypeSubstitutor\n\
                  \x20   implements InferenceTypeVisitor<TypeImpl2> {\n\
                  \x20 TypeImpl2 visitUnknownInferredType(UnknownInferredType t) =>\n\
                  \x20     TypeImpl2();\n\
                  }\n\
                  class TopSubstitutor extends TypeSubstitutor {}\n";
    let names = code_names(&report(source));
    assert!(
        override_family_clean(&names),
        "the same-file generic edge stays silent: {names:?}"
    );
}
