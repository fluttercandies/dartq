//! The #2325 differential pins: a mixin application's member face reads
//! through the application's written type arguments, not the mixin's own
//! parameter spellings. The burn-down shape (xml's
//! `mixins/has_children.dart`): `mixin XmlHasChildren<T extends XmlNode>`
//! declares `XmlNodeList<T> children`; `class XmlElement extends XmlNode
//! with XmlHasChildren<XmlNode>` applies the mixin with `XmlNode`, so a
//! read of `element.children` types as `XmlNodeList<XmlNode>` — the
//! `XmlNodeList<T>` face the un-substituted walk used to keep (the oracle's
//! `inheritance_manager3` substitutes the declaring signature through the
//! written `with` arguments). The environment composes at the member-table
//! fold ([`Engine::walk_from`]'s mixin branch) through the same
//! `mixin_env`/`substitute_entry` machinery the override face already
//! reads — a foreign application class whose clause facts live outside the
//! diagnosing file stays undecidable and keeps the under-report posture.
//!
//! Each fixture was probed against oracle `dart analyze` (fvm default) via
//! the isolated `/tmp/probe2325` trees: the substituted member face types
//! the receiver text, the callable-chain argument check, and the operator
//! head row identically.

use super::*;
use crate::collector::collect;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_resolution::{Combinators, ImportEdge, LibraryGraphBuilder};
use dartq_scanner::ScanConfig;
use dartq_source::FileTable;
use std::path::Path;

/// The minimal two-file closure: `main.dart` applies a generic mixin
/// declared in `mix.dart` and reads the substituted member face. Diagnoses
/// `main.dart` and returns its report.
fn report(main: &str, mix: &str) -> FileReport {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file_main = files.intern(Path::new("main.dart"));
    let file_mix = files.intern(Path::new("mix.dart"));
    let scan = |source: &str, file: FileId| {
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
        scanned
    };
    let scanned_main = scan(main, file_main);
    let scanned_mix = scan(mix, file_mix);
    let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
    let features = ParserFeatures::from_feature_set(set);
    let elements_main = collect(main, &scanned_main.tokens, features, &strings);
    let elements_mix = collect(mix, &scanned_mix.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib_main = builder.intern("package:t/main.dart");
    let lib_mix = builder.intern("package:t/mix.dart");
    let mut index = ElementIndex::default();
    index.insert(file_main, elements_main.clone());
    index.insert(file_mix, elements_mix.clone());
    for (lib, file, elements) in [
        (lib_main, file_main, &elements_main),
        (lib_mix, file_mix, &elements_mix),
    ] {
        for (name, decl) in elements.declarations() {
            if decl.kind == MicroKind::TypeParam {
                continue;
            }
            builder.declare(lib, name, file);
        }
    }
    builder.import(
        lib_main,
        ImportEdge {
            target: Some(lib_mix),
            prefix: None,
            deferred: false,
            combinators: Combinators::unfiltered(),
        },
    );
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    let fold = fold_unit(
        main,
        &scanned_main.tokens,
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

fn diagnostics_for<'a>(report: &'a FileReport, name: &str) -> Vec<&'a Diagnostic> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == name)
        .collect()
}

/// The foreign mixin's declaring file: `XmlNode` is the constraint and the
/// substitution target, `XmlNodeList<E>` the member's container type, and
/// `XmlHasChildren<T>` carries `XmlNodeList<T> children` — the signature
/// the application must instantiate.
const MIX: &str = "class XmlNode {}\n\
                   class XmlNodeList<E extends XmlNode> {}\n\
                   mixin XmlHasChildren<T extends XmlNode> {\n\
                   \x20 final XmlNodeList<T> children = XmlNodeList<T>();\n\
                   }";

/// The application face in the diagnosing file: `XmlElement extends
/// XmlNode with XmlHasChildren<XmlNode>` — the `with` clause's written
/// argument `XmlNode` substitutes for the mixin's `T`. A member read that
/// the substituted `XmlNodeList<XmlNode>` lacks reports `undefined_getter`
/// with the substituted type text (oracle probe e: `'first' isn't defined
/// for the type 'XmlNodeList<XmlNode>'`, never `'XmlNodeList<T>'`).
#[test]
fn mixin_application_member_reads_through_written_arguments() {
    let main = "import 'mix.dart';\n\
                class XmlElement extends XmlNode with XmlHasChildren<XmlNode> {}\n\
                void use(XmlElement e) {\n\
                \x20 e.children.first;\n\
                }";
    let rep = report(main, MIX);
    let getter = diagnostics_for(&rep, "undefined_getter");
    assert_eq!(getter.len(), 1, "{:?}", rep.diagnostics);
    assert!(
        getter[0]
            .message
            .contains("the type 'XmlNodeList<XmlNode>'"),
        "{:?}",
        getter[0].message
    );
}

/// The un-substituted control: without the application's argument the
/// member face would type `children` as `XmlNodeList<T>` — the message's
/// type text is the proof the substitution ran, so the pin asserts the
/// mixin's own parameter spelling never reaches the read.
#[test]
fn mixin_parameter_spelling_never_reaches_the_member_face() {
    let main = "import 'mix.dart';\n\
                class XmlElement extends XmlNode with XmlHasChildren<XmlNode> {}\n\
                void use(XmlElement e) {\n\
                \x20 e.children.first;\n\
                }";
    let rep = report(main, MIX);
    for d in &rep.diagnostics {
        assert!(
            !d.message.contains("XmlNodeList<T>"),
            "{:?}",
            d.message
        );
    }
}

/// Diagnoses a single-file library with no imports — the face-②/③ pins
/// need no cross-file wiring.
fn report_single(source: &str) -> FileReport {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
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

/// Face ② (string.dart's `_evalMatches(_asString(...), p, null)` shape): a
/// callable-chain argument keeps its declared type through the chain — a
/// `String` argument into a `String` parameter stays silent while a
/// `String?` into the same parameter reports `argument_type_not_assignable`
/// (oracle: exactly the nullable arg reports). The pin guards the chain's
/// receiver typing from narrowing a declared-typed argument mid-send.
#[test]
fn callable_chain_argument_keeps_its_declared_type() {
    let source = "class Item {}\n\
                  \x20class Seq {\n\
                  \x20  List<Item> atomize() => [];\n\
                  \x20}\n\
                  \x20String? asStr(Item? i) => null;\n\
                  \x20void eval(String? a, String b, String? c) {}\n\
                  \x20class String {}\n\
                  \x20class List<T> { T get first => throw 0; }\n\
                  \x20void take(Seq input, String p, String? r) {\n\
                  \x20  eval(asStr(input.atomize().first), p, null);\n\
                  \x20  eval(asStr(input.atomize().first), r, null);\n\
                  \x20}";
    let rep = report_single(source);
    let assignable = diagnostics_for(&rep, "argument_type_not_assignable");
    assert_eq!(assignable.len(), 1, "{:?}", rep.diagnostics);
    // The reporting argument is the nullable `r` in the second send.
    assert_eq!(
        assignable[0].range.offset as usize,
        source.find(", r, null").unwrap() + 2,
        "{:?}",
        rep.diagnostics
    );
}

/// Face ③ (the operator arm's head emission): an undefined operand in an
/// operator application reports `undefined_identifier` at the head's own
/// span — `missing + 1` anchors `missing` (oracle: the same row, same
/// column), never the operator's span.
#[test]
fn operator_arm_reports_the_undefined_head() {
    let source = "void use() {\n\
                  \x20  var x = missing + 1;\n\
                  \x20}";
    let rep = report_single(source);
    let undefined = diagnostics_for(&rep, "undefined_identifier");
    assert_eq!(undefined.len(), 1, "{:?}", rep.diagnostics);
    assert_eq!(
        undefined[0].range.offset as usize,
        source.find("missing").unwrap(),
        "{:?}",
        rep.diagnostics
    );
    // No operator-face row joins the head emission — the operand never
    // typed, so `undefined_operator` must not fire on `+`.
    assert!(
        diagnostics_for(&rep, "undefined_operator").is_empty(),
        "{:?}",
        rep.diagnostics
    );
}
