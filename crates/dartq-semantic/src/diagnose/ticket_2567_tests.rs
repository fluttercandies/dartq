//! The #2567 pins: assignability matrix sweep — fill in as roots close. The rig
//! declares no imports, so every pin spells custom types only.

use super::*;
use crate::collector::collect;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_resolution::LibraryGraphBuilder;
use dartq_scanner::ScanConfig;
use dartq_source::FileTable;
use std::path::Path;

/// Diagnoses `source` as the sole file of a library with no imports —
/// the single-file rig (`ticket_2564_tests`' shape).
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

fn count_code(report: &FileReport, name: &str) -> usize {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == name)
        .count()
}

fn messages(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .map(|d| format!("{}: {}", d.code.name, d.message))
        .collect()
}


/// The two-file rig: `main_src` imports `other_src` under the prefix
/// `p` — the prefixed-callee seam (`p.f(x)` resolving through the
/// import table into the other unit's element model).
fn report_prefixed(other_src: &str, main_src: &str) -> FileReport {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
    let other_file = files.intern(Path::new("other.dart"));
    let scan = |src: &str| {
        let scanned = dartq_scanner::scan(
            src,
            ScanConfig {
                enable_triple_shift: true,
                enable_augmentations: true,
            },
            file,
        );
        assert!(scanned.diagnostics.is_empty(), "scanner diagnostics: {src:?}");
        scanned
    };
    let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
    let features = ParserFeatures::from_feature_set(set);
    let scanned = scan(main_src);
    let other_scanned = scan(other_src);
    let elements = collect(main_src, &scanned.tokens, features, &strings);
    let other_elements = collect(other_src, &other_scanned.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib = builder.intern("package:t/main.dart");
    let other_lib = builder.intern("package:t/other.dart");
    let mut index = ElementIndex::default();
    index.insert(file, elements.clone());
    index.insert(other_file, other_elements.clone());
    for (name, decl) in elements.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(lib, name, file);
    }
    for (name, decl) in other_elements.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(other_lib, name, other_file);
    }
    builder.import(
        lib,
        dartq_resolution::library_graph::ImportEdge {
            target: Some(other_lib),
            prefix: Some(strings.intern("p")),
            deferred: false,
            combinators: dartq_resolution::library_graph::Combinators::unfiltered(),
        },
    );
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    let fold = fold_unit(
        main_src,
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

/// The corpus `??=` shape (`filename ??= path.basename(file.path)` —
/// archive's zip encoder): a prefixed callee's concrete return types
/// the assignment compatible — no invalid-assignment face, and the
/// prefix itself never leaks as the RHS type face.
#[test]
fn prefixed_callee_return_feeds_the_assign_rebind() {
    let report = report_prefixed(
        "class Payload { }\nPayload pick(Payload n) => n;\n",
        "import 'other.dart' as p;\n\
         class Holder {\n\
           p.Payload? field;\n\
           void f(p.Payload n) {\n\
             field ??= p.pick(n);\n\
             field.missing;\n\
           }\n\
         }\n",
    );
    assert!(
        report.diagnostics.is_empty(),
        "prefixed-callee ??= mis-reports: {:?}",
        messages(&report)
    );
}

/// A generic prefixed callee's free inference parameter never escapes
/// as the caller-scope type: `field ??= p.pick(a, b)` answers the
/// unknown face (the under-report direction until call-site inference
/// solves it), so no downstream sink reports against the raw `T`
/// spelling (the analyzer `math.min` family).
#[test]
fn prefixed_generic_callee_frees_to_unknown() {
    let report = report_prefixed(
        "class Payload { }\nT pick<T extends num>(T a, T b) => a;\n",
        "import 'other.dart' as p;\n\
         class Holder {\n\
           p.Payload? field;\n\
           void f(p.Payload a, p.Payload b) {\n\
             field ??= p.pick(a, b);\n\
             field.missing;\n\
           }\n\
         }\n",
    );
    assert!(
        !messages(&report).iter().any(|m| m.contains("'T'")),
        "raw schema spelling visible: {:?}",
        messages(&report)
    );
}

/// A foreign generic callee's parameter spelling is recovered even when
/// an unrelated declaration in THIS file declares a type parameter of
/// the same name: `pick`'s `T` is not `acc<T>`'s `T` — the recovery
/// proves leaves from the call site's scope (the frame latch), never the
/// file's global declaration table, so the free parameter still
/// collapses to the unknown face (analyzer's `single_file_byte_store`
/// `min` family: the file's own `withExclusiveAccess<T>` methods
/// emptied the recovery and the raw `T` leaked into every sink).
#[test]
fn foreign_param_recovery_ignores_same_file_type_param_decls() {
    let report = report_prefixed(
        "class Payload { }\nT pick<T extends num>(T a, T b) => a;\n",
        "import 'other.dart' as p;\n\
         class Unrelated {\n\
           T acc<T>(T Function() f) => f();\n\
         }\n\
         class Pool {\n\
           void read() {\n\
             var n = p.pick(1, 2);\n\
             int o = 3 + n;\n\
             print(o);\n\
           }\n\
         }\n",
    );
    assert!(
        !messages(&report).iter().any(|m| m.contains("'T'")),
        "raw schema spelling visible: {:?}",
        messages(&report)
    );
}

/// A `??=` whose right side is a numeric binary (`length ??= 5 -
/// offset`) promotes the target: the fold's left operand is a literal
/// row typed from its own token bytes, the binary's result reaches the
/// write staging, and the join writes the non-null face (the corpus
/// `min((end - first) * pageSize, ...)` / `length ??= 5 - offset`
/// family). The rig seeds the implicit dart:core names (the bare rig
/// would report `int` itself undefined).
#[test]
fn if_null_rhs_binary_promotes_target() {
    let report = report(
        "class Sink { void take(int x) {} }\n\
         void f(int? offset, int? length) {\n\
           offset ??= 0;\n\
           length ??= 5 - offset;\n\
           Sink().take(length);\n\
           Sink().take(offset + length);\n\
         }\n",
    );
    let report = FileReport {
        diagnostics: report
            .diagnostics
            .into_iter()
            .filter(|d| d.code.name != "undefined_class")
            .collect(),
        ..report
    };
    assert!(
        report.diagnostics.is_empty(),
        "??= binary-RHS promotion missing: {:?}",
        messages(&report)
    );
}

/// A `??=` whose right side is a generic call the site solves
/// (`x ??= pick(a, b)` over `T pick<T>(T a, T b)`) promotes the target:
/// the send-close upward inference binds `T := W` from the typed
/// arguments, the instantiated return reaches the write staging, and
/// the join writes the non-null face (the corpus `math.max(0, n - 1)`
/// family — same-file spelling here; the prefixed face rides the same
/// solver through the import table). `W y = x` observably needs the
/// promotion — an unpromoted `W?` writes `invalid_assignment`.
#[test]
fn if_null_rhs_generic_call_promotes_target() {
    let report = report(
        "class W {}\n\
         T pick<T>(T a, T b) => a;\n\
         W sink(W v) => v;\n\
         void f(W? x, W a, W b) {\n\
           x ??= pick(a, b);\n\
           W y = x;\n\
           sink(y);\n\
         }\n",
    );
    assert!(
        report.diagnostics.is_empty(),
        "??= generic-call-RHS promotion missing: {:?}",
        messages(&report)
    );
}
