//! The prefixed-type completion family (#2536): the official
//! `PrefixedType.parseType` protocol hands the semantic layer the PREFIX
//! token in `handleType` and the continuation leaf separately, so the
//! armed head must carry the dot-joined spelling (`io.File`) — the intern
//! table's one-head form — while every name-keyed consumer (the flow
//! registry's alias keys, the container index, the enum coverage space)
//! keeps the dot-free declaration name. The pins hold the two invariants
//! the family's faces depend on: the joined alias must never strand a
//! declaration's supertype edges (the registry inversion keys on the
//! declaration name), and the prefix-reached enum constant must cover in
//! the switch space.
//!
//! Oracle provenance (`dart analyze` 3.13.1): a bare-and-prefixed double
//! import of one library keeps `is`-promotions live (`state is
//! LibraryImportWithFile && !state.importedFile.exists` reports nothing
//! — the promoted face reads the subclass member), a prefixed negated
//! `is!` with early return promotes the trailing reads exactly like the
//! bare form, a prefixed-constant enum switch covers (`case p.Level.a`)
//! when every constant appears, and the sealed is-chain's else-branches
//! stay reachable (no `dead_code`).

use crate::collector::collect;
use crate::diagnose::{FileReport, StrictModes, diagnose_with};
use crate::library::ElementIndex;
use crate::model::MicroKind;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_resolution::{
    Combinators, ImportEdge, LibraryClassModifiers, LibraryGraphBuilder, Namespaces,
};
use dartq_scanner::ScanConfig;
use dartq_source::{FileTable, StringTable};
use std::path::Path;

/// The double-import rig: `package:t/main.dart` imports `kit.dart` BOTH
/// bare and prefixed (the analyzer-corpus shape — `file_state.dart`
/// twice), and returns the main file's report. `prefix` also allows a
/// single prefixed edge (`None` = one bare edge) for the single-face
/// controls.
fn double_import_report(main_source: &str, dependency: &str, prefix: Option<&str>) -> FileReport {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
    let dep_file = files.intern(Path::new("kit.dart"));
    let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
    let features = ParserFeatures::from_feature_set(set);
    let scanned = dartq_scanner::scan(
        main_source,
        ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: true,
        },
        file,
    );
    assert!(
        scanned.diagnostics.is_empty(),
        "scanner diagnostics: {main_source:?}"
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
    let elements = collect(main_source, &scanned.tokens, features, &strings);
    let dep_elements = collect(dependency, &scanned_dep.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib = builder.intern("package:t/main.dart");
    let dep_lib = builder.intern("package:t/kit.dart");
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
            prefix: None,
            deferred: false,
            combinators: Combinators::unfiltered(),
        },
    );
    if let Some(p) = prefix {
        builder.import(
            lib,
            ImportEdge {
                target: Some(dep_lib),
                prefix: Some(strings.intern(p)),
                deferred: false,
                combinators: Combinators::unfiltered(),
            },
        );
    }
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    let mut index = ElementIndex::default();
    index.insert(file, elements);
    index.insert(dep_file, dep_elements);
    diagnose_with(
        main_source,
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

fn codes(report: &FileReport) -> Vec<&'static str> {
    report.diagnostics.iter().map(|d| d.code.name).collect()
}

/// The analyzer-corpus shape (`library_analyzer.dart:750-758`): the
/// double import registers every container once, the joined alias keys
/// (`p.LibraryImportWithFile`) must not strand the declaration's
/// supertype edges — the else-if promotion keeps reading the subclass
/// member and the then-branch keeps the base member, no
/// `undefined_getter`, no `dead_code`.
#[test]
fn double_import_promotion_keeps_supertype_edges() {
    let report = double_import_report(
        r#"
import 'package:t/kit.dart';
import 'package:t/kit.dart' as p;

void h(LibraryImportState state) {
  if (state is LibraryImportWithUri) {
    state.importedSource;
  } else if (state is LibraryImportWithFile && state.importedFile.exists) {
    state.importedFile;
  }
}
"#,
        r#"
sealed class LibraryImportState {}
sealed class LibraryImportWithUri extends LibraryImportState {
  FileState get importedSource => FileState();
}
final class LibraryImportWithFile extends LibraryImportWithUri {
  FileState get importedFile => FileState();
}
final class FileState {
  bool exists = true;
}
"#,
        Some("p"),
    );
    let cs = codes(&report);
    assert!(
        !cs.contains(&"undefined_getter"),
        "undefined_getter leaked: {cs:?}"
    );
    assert!(!cs.contains(&"dead_code"), "dead_code leaked: {cs:?}");
}

/// The prefixed negated test (`is! p.X`) with early return promotes the
/// trailing reads exactly like the bare form: the joined armed head
/// resolves through the alias face and the receiver reads the subclass
/// member.
#[test]
fn prefixed_negated_is_promotes_after_early_return() {
    let report = double_import_report(
        r#"
import 'package:t/kit.dart' as p;

void g(p.LibraryImportState state) {
  if (state is! p.LibraryImportWithFile) {
    return;
  }
  state.importedFile;
}
"#,
        r#"
sealed class LibraryImportState {}
final class LibraryImportWithFile extends LibraryImportState {
  FileState get importedFile => FileState();
}
final class FileState {}
"#,
        Some("p"),
    );
    assert!(
        !codes(&report).contains(&"undefined_getter"),
        "codes: {:?}",
        codes(&report)
    );
}

/// The prefix-reached enum constant covers in the switch space
/// (`android_webview.ConsoleMessageLevel.debug`): a fully covered
/// prefixed-constant switch reports nothing, and the one-constant
/// control keeps reporting — the pin proves the coverage, not a
/// suppressed check.
#[test]
fn prefixed_enum_constant_cases_cover() {
    let full = double_import_report(
        r#"
import 'package:t/kit.dart';
import 'package:t/kit.dart' as p;

void f(Msg m) {
  switch (m.level) {
    case p.Level.a:
      m.level;
    case p.Level.b:
      m.level;
  }
}
"#,
        "enum Level { a, b }\n\nclass Msg { final Level level; Msg(this.level); }",
        Some("p"),
    );
    assert!(
        !codes(&full).contains(&"non_exhaustive_switch_statement"),
        "full cover reported: {:?}",
        codes(&full)
    );
    let partial = double_import_report(
        r#"
import 'package:t/kit.dart';
import 'package:t/kit.dart' as p;

void f(Msg m) {
  switch (m.level) {
    case p.Level.a:
      m.level;
  }
}
"#,
        "enum Level { a, b }\n\nclass Msg { final Level level; Msg(this.level); }",
        Some("p"),
    );
    assert!(
        codes(&partial).contains(&"non_exhaustive_switch_statement"),
        "partial control stayed silent: {:?}",
        codes(&partial)
    );
}

/// The sealed is-chain over prefixed spellings (`fs.dart`'s DirectiveUri
/// hierarchy): every else-if branch stays reachable — the factored
/// remainder of one test never swallows the next test's arm, no
/// `dead_code` on the branches.
#[test]
fn prefixed_sealed_is_chain_stays_reachable() {
    let report = double_import_report(
        r#"
import 'package:t/kit.dart' as p;

class Impl extends p.DirectiveUri {}

Impl f(p.DirectiveUri self) {
  if (self is p.WithSource) {
    return Impl();
  } else if (self is p.WithUri) {
    return Impl();
  } else if (self is p.WithString) {
    return Impl();
  }
  return Impl();
}
"#,
        r#"
sealed class DirectiveUri {}
final class WithSource extends DirectiveUri {}
final class WithString extends DirectiveUri {}
final class WithUri extends WithString {}
"#,
        Some("p"),
    );
    assert!(
        !codes(&report).contains(&"dead_code"),
        "dead_code leaked: {:?}",
        codes(&report)
    );
}
