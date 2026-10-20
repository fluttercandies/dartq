//! The #2326 differential pins: the with-clause override producer wired
//! from `handle_class_with_clause` / `handle_named_mixin_application_
//! with_clause` / `handle_enum_with_clause` — upstream
//! `InheritanceOverrideVerifier.verify` (upstream Dart SDK source,
//! `pkg/analyzer/lib/src/error/inheritance_override.dart:338-364`) checks
//! every mixin member against the accumulated set `[superclass] +
//! prior mixins`, never `implements`/`on`, and anchors the row at the
//! written `NamedType` span (type arguments included). Each fixture was
//! probed against oracle `dart analyze` (fvm default) via the isolated
//! `xmlrepro` tree.

use super::*;
use crate::collector::collect;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_resolution::{Combinators, ImportEdge, LibraryGraphBuilder};
use dartq_scanner::ScanConfig;
use dartq_source::FileTable;
use std::path::Path;

/// Diagnoses `source` as the sole file of a library with no imports —
/// the single-file rig, local to this module.
fn report(source: &str) -> FileReport {
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

/// Diagnoses `main_source` as one file of a two-file library — `mix_source`
/// declares the mixin `main` applies, foreign to `main`'s diagnosis pool.
fn report_two_file(main_source: &str, mix_source: &str) -> FileReport {
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
    let scanned_main = scan(main_source, file_main);
    let scanned_mix = scan(mix_source, file_mix);
    let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
    let features = ParserFeatures::from_feature_set(set);
    let elements_main = collect(main_source, &scanned_main.tokens, features, &strings);
    let elements_mix = collect(mix_source, &scanned_mix.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib_main = builder.intern("package:t/main.dart");
    let lib_mix = builder.intern("package:t/mix.dart");
    let mut index = ElementIndex::default();
    index.insert(file_main, elements_main.clone());
    index.insert(file_mix, elements_mix.clone());
    for (name, decl) in elements_main.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(lib_main, name, file_main);
    }
    for (name, decl) in elements_mix.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(lib_mix, name, file_mix);
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
        main_source,
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

fn code_names(report: &FileReport) -> Vec<&'static str> {
    report
        .diagnostics
        .iter()
        .map(|d| d.code.name)
        .collect::<Vec<_>>()
}

/// Diagnoses `source` as the sole file of a library with no imports through
/// the whole pipeline — `finalize_usage` and its library-level passes
/// included (`fold_unit` stops before them).
fn report_full(source: &str) -> FileReport {
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
    let (report, _) = diagnose_unit(
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
    report
}

/// (a) The written `NamedType` span anchors the row — the `<XmlNode>`
/// argument tail sits inside the anchor, not after it (`element.dart:19:9
/// len23` in the frozen key = `XmlHasChildren<XmlNode>` end-to-end). The
/// broken member is the getter: `M<Other>`'s `Other children` cannot
/// override `B`'s `Base children` (return covariance between two
/// unrelated declared classes).
#[test]
fn mixin_row_anchors_at_the_named_type_with_its_args() {
    let source = "class Base {}\n\
                  class Other {}\n\
                  class B {\n\
                  \x20 Base get children => throw '';\n\
                  }\n\
                  mixin XmlHasChildren<T> {\n\
                  \x20 T get children => throw '';\n\
                  }\n\
                  class XmlNode extends B {}\n\
                  class XmlElement extends XmlNode with XmlHasChildren<Other> {}\n";
    let rep = report(source);
    let names = code_names(&rep);
    assert!(
        names.contains(&"invalid_override"),
        "the mixin-application row must fire: {names:?}"
    );
    let row = rep
        .diagnostics
        .iter()
        .find(|d| d.code.name == "invalid_override")
        .expect("row");
    // The `XmlHasChildren<Other>` spelling starts at the with-clause's
    // first entry; the anchor runs the whole `NamedType` end-to-end — head
    // plus the `<...>` argument list, closing `>` included (the oracle's
    // own convention: `element.dart:19:9` spans all 23 chars of
    // `XmlHasChildren<XmlNode>`).
    let at = source.find("XmlHasChildren<Other>").unwrap() as u32;
    assert_eq!(row.range.offset, at, "anchor starts at the mixin name");
    assert_eq!(
        row.range.length, 21,
        "anchor covers the whole NamedType incl. the `<Other>` tail"
    );
}

/// (b) A mixin's non-final field rides BOTH its implicit getter and its
/// implicit setter into the check: a superclass carrying a clashing `x`
/// reports once per face (upstream `_checkDeclaredMembers` iterates
/// `type.getters` then `type.setters` — the field contributes a face to
/// each pool). `M.x` = `String` against `Base.x` = `int`: getter covariance
/// AND setter contravariance both break.
#[test]
fn mixin_field_emits_both_getter_and_setter_faces() {
    let source = "class Base {\n\
                  \x20 int get x => 0;\n\
                  \x20 set x(int v) {}\n\
                  }\n\
                  mixin M {\n\
                  \x20 String x = '';\n\
                  }\n\
                  class C extends Base with M {}\n";
    let rep = report(source);
    let names = code_names(&rep);
    let overrides: Vec<_> = rep
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "invalid_override")
        .collect();
    // The getter arm (`String` is not a subtype of `int`) plus the setter
    // arm (`String` is not a supertype of `int` — parameter contravariance)
    // both report at the `M` span. Both rows share the `invalid_override`
    // code; the setter face prefixes its message with "The setter".
    assert_eq!(
        overrides.len(),
        2,
        "getter + setter faces each report: {names:?}"
    );
    assert!(
        overrides
            .iter()
            .any(|d| d.message.starts_with("The setter")),
        "a setter-shaped row must report: {:?}",
        overrides.iter().map(|d| &d.message).collect::<Vec<_>>()
    );
    assert!(
        overrides
            .iter()
            .any(|d| !d.message.starts_with("The setter")),
        "a getter-shaped row must report: {:?}",
        overrides.iter().map(|d| &d.message).collect::<Vec<_>>()
    );
}

/// (c) The accumulated set is `[superclass] + prior mixins`, never
/// `implements`/`on`: `with M1, M2` checks `M2`'s `m` against `M1`'s `m`
/// (the prior mixin in accumulation order), not against `I.m` — even
/// though `C implements I` names the same member. Upstream adds
/// `element.interfaces` only AFTER the mixin loop.
#[test]
fn mixin_checks_prior_mixins_not_interfaces() {
    let source = "abstract interface class I { void m(int v); }\n\
                  mixin M1 { void m(num v) {} }\n\
                  mixin M2 { void m(int v) {} }\n\
                  class C implements I with M1, M2 {}\n";
    let rep = report(source);
    let names = code_names(&rep);
    // `M1.m(num)` accepts `int`, so `M2.m(int)` is a valid override of the
    // prior mixin's member — and the interface's `I.m(int)` never enters
    // the check set at all. The row stays silent.
    assert!(
        !names.contains(&"invalid_override"),
        "M2.m(int) is a valid override of M1.m(num): {names:?}"
    );
}

/// (d) A foreign single-parameter mixin instantiates `T ↦ XmlText` from
/// the written application argument — the name-forced hop's only
/// decidable shape (upstream `member.typeParameters` vs `mixinType
/// .typeArguments`; a multi-parameter foreign hop stays undecidable and
/// suppresses, the under-report direction).
///
/// Scope posture: every declaring-side read resolves in the mixin's own
/// file, so the substituted spelling must be visible there — an
/// application-site-only name leaves the declared head invalid and the
/// face suppresses. Both fixture names are therefore declared where the
/// mixin lives and the application argument is written in `main.dart`;
/// without the `T ↦ XmlText` substitution the declared signature would
/// render `T Function()`, which is unresolvable in the mixin's scope and
/// silent.
#[test]
fn foreign_mixin_single_param_instantiates_from_application_args() {
    let mix = "class XmlText {}\n\
               class XmlNodeBase {\n\
               \x20 XmlNodeBase get children => throw XmlNodeBase();\n\
               }\n\
               mixin XmlHasChildren<T> {\n\
               \x20 T get children => throw XmlText();\n\
               }\n";
    let main = "import 'mix.dart';\n\
                class XmlNode extends XmlNodeBase {}\n\
                class XmlElement extends XmlNode\n\
                \x20 with XmlHasChildren<XmlText> {}\n";
    let rep = report_two_file(main, mix);
    let names = code_names(&rep);
    assert!(
        names.contains(&"invalid_override"),
        "the foreign mixin row must fire with T↦XmlText instantiated: {names:?}"
    );
    let row = rep
        .diagnostics
        .iter()
        .find(|d| d.code.name == "invalid_override")
        .expect("row");
    let at = main.find("XmlHasChildren<XmlText>").unwrap() as u32;
    assert_eq!(row.range.offset, at);
    // The whole `NamedType` end-to-end, closing `>` included (the same
    // convention the in-file pin above locks).
    assert_eq!(row.range.length, 23);
    // The declared-side signature renders with `XmlText`, not `T` — the
    // substitution reached into the member's `T`-carrying slots.
    assert!(
        row.message.contains("XmlText"),
        "the declared side instantiates at the application site: {:?}",
        row.message
    );
}

/// (e) A deprecated SDK library reports once, at the whole import directive
/// — `usageRange: node.sourceRange` on the ImportDirective (upstream
/// `element_usage_detector.dart:256`), which for `import 'dart:html';` is
/// offset 0 / len 19, never the URI literal's offset 7. Probed against the
/// oracle: INFO-severity `HINT` rows carry the normalized annotation tail
/// and the fixed replacement correction.
#[test]
fn deprecated_sdk_import_anchors_the_whole_directive() {
    let source = "import 'dart:html';\n\nvoid main() {}\n";
    let rep = report_full(source);
    let row = rep
        .diagnostics
        .iter()
        .find(|d| d.code.name == "deprecated_member_use")
        .unwrap_or_else(|| panic!("row missing: {:?}", code_names(&rep)));
    assert_eq!(row.code.dtype.as_str(), "HINT");
    assert_eq!(row.severity, dartq_diagnostics::Severity::Info);
    assert_eq!((row.range.offset, row.range.length), (0, 19));
    assert_eq!(
        row.message.as_str(),
        "'dart:html' is deprecated and shouldn't be used. \
         Use package:web and dart:js_interop instead."
    );
    assert_eq!(
        row.correction.as_deref(),
        Some("Try replacing the use of the deprecated member with the replacement.")
    );
}

/// (f) The export face rides the same check (upstream runs `checkUsage` on
/// export directives too): `export 'dart:svg';` reports at offset 0 / len
/// 18 — the ExportDirective's own span.
#[test]
fn deprecated_sdk_export_anchors_the_whole_directive() {
    let source = "export 'dart:svg';\n";
    let rep = report_full(source);
    let row = rep
        .diagnostics
        .iter()
        .find(|d| d.code.name == "deprecated_member_use")
        .unwrap_or_else(|| panic!("row missing: {:?}", code_names(&rep)));
    assert_eq!((row.range.offset, row.range.length), (0, 18));
    assert_eq!(
        row.message.as_str(),
        "'dart:svg' is deprecated and shouldn't be used. \
         Use package:web and dart:js_interop instead."
    );
}

/// (g) The annotation text normalizes exactly like upstream
/// `normalizeDeprecationMessage`: `dart:cli`'s message carries no terminal
/// punctuation, so the rendered tail gains one period — the oracle's
/// `'dart:cli' is deprecated and shouldn't be used. The functionality of
/// this library is incomplete and may be removed in a later version.`
#[test]
fn deprecated_sdk_annotation_message_gains_the_terminal_period() {
    let source = "import 'dart:cli';\n\nvoid main() {}\n";
    let rep = report_full(source);
    let row = rep
        .diagnostics
        .iter()
        .find(|d| d.code.name == "deprecated_member_use")
        .unwrap_or_else(|| panic!("row missing: {:?}", code_names(&rep)));
    assert_eq!(
        row.message.as_str(),
        "'dart:cli' is deprecated and shouldn't be used. The functionality of \
         this library is incomplete and may be removed in a later version."
    );
}

/// (h) The resolved-library gate: `dart:web_sql` sits on disk in the SDK
/// tree but is tabulated by neither the flat `dart:` table nor the
/// deprecation metadata's own consumers, so the URI never resolves and the
/// oracle reports nothing — the row must stay silent on both faces.
#[test]
fn unresolvable_dart_uri_reports_no_deprecation() {
    for source in [
        "import 'dart:web_sql';\n\nvoid main() {}\n",
        "export 'dart:web_sql';\n",
    ] {
        let rep = report_full(source);
        let deprecations: Vec<_> = rep
            .diagnostics
            .iter()
            .filter(|d| d.code.name == "deprecated_member_use")
            .collect();
        assert!(deprecations.is_empty(), "{source:?}");
    }
}
