//! The #2482 pins: the discarded-value face (`unused_result`) binds to the
//! DECLARATION a reference resolved to, never to its spelling.
//!
//! The official verifier reads the resolved element's own metadata
//! (upstream Dart SDK source, pkg/analyzer/lib/src/error/
//! use_result_verifier.dart `_getUseResultMetadata`), so a lexical binding
//! or the enclosing container's own member of the same spelling as a
//! `@useResult`-annotated top-level declaration owns the reference and the
//! row stays silent. The census rows this ticket re-routes are the
//! petitparser shapes: `position.dart` 22:51 (a formal parameter read —
//! `int fastParseOn(String buffer, int position) => position;`),
//! `predicate.dart` 57:53 (an instance field read in argument position —
//! `PredicateParser(length, predicate, message)`) and `predicate.dart`
//! 63:7 (an instance field read in a comparison —
//! `predicate == other.predicate`), all beside same-spelled annotated
//! top-level declarations. Every expectation below is a verbatim oracle
//! 3.13.1 row (`dart analyze`, package:meta 1.19.0).

use super::super::{FileReport, StrictModes, diagnose_with};
use crate::collector::collect;
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

/// The meta-import rig: `main.dart` is the analyzed unit, `meta.dart`
/// declares the `useResult` getter and is imported unfiltered, so the
/// annotation's meta provenance resolves (the corpus's package:meta
/// posture). No other library is mounted — a fixture declares every
/// member-bearing class it names.
fn meta_report(main_source: &str) -> FileReport {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
    let core_file = files.intern(Path::new("<dart:core>"));
    let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
    let features = ParserFeatures::from_feature_set(set);
    let scan = |source: &str, file| {
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
    let scanned = scan(main_source, file);
    let elements = collect(main_source, &scanned.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib = builder.intern("package:t/main.dart");
    for (name, decl) in elements.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(lib, name, file);
    }
    builder.seed_implicit_core(
        lib,
        core_file,
        &["Object", "String", "int", "bool", "double", "num"],
    );
    let mut index = ElementIndex::default();
    index.insert(file, elements);
    let dep_source = "library meta;\nbool get useResult => true;\n";
    let dep_file = files.intern(Path::new("meta.dart"));
    let dep_lib = builder.intern("package:t/meta.dart");
    builder.library_name(dep_lib, "meta");
    let scanned_dep = scan(dep_source, dep_file);
    let dep_elements = collect(dep_source, &scanned_dep.tokens, features, &strings);
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
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
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

/// The `unused_result` rows of `source`'s report, as (offset, length)
/// pairs. The fixtures are ASCII, so the row spans index the source bytes
/// directly (the diagnostic offsets are UTF-16 — equal under ASCII).
fn unused_result_rows(source: &str) -> Vec<(u32, u32)> {
    meta_report(source)
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "unused_result")
        .map(|d| (d.range.offset, d.range.length))
        .collect()
}

/// `position.dart` 22:51: `int fastParseOn(String buffer, int position) =>
/// position;` beside the annotated top-level `position()`. The read binds
/// the FORMAL PARAMETER, which carries no metadata — the oracle is silent
/// (probe2482 a: `No issues`).
#[test]
fn formal_parameter_shadow_stays_silent() {
    let source = "\
import 'package:t/meta.dart';

@useResult
Position position() => Position();

class Position {}

class PositionParser extends Position {
  int fastParseOn(String buffer, int position) => position;
}
";
    assert!(unused_result_rows(source).is_empty(), "{source:?}");
}

/// `predicate.dart` 57:53: `PredicateParser(length, predicate, message)`
/// inside `copy()`. The argument read binds the instance FIELD, not the
/// annotated top-level `predicate` function — the oracle is silent
/// (probe2482 b: `No issues`).
#[test]
fn field_shadow_in_argument_position_stays_silent() {
    let source = "\
import 'package:t/meta.dart';

@useResult
Predicate predicate(int length, Predicate predicate) =>
    Predicate(length, predicate);

class Predicate {
  Predicate(this.length, this.predicate);

  final int length;
  final Predicate predicate;

  Predicate copy() => Predicate(length, predicate);
}
";
    assert!(unused_result_rows(source).is_empty(), "{source:?}");
}

/// `predicate.dart` 63:7: `predicate == other.predicate` inside
/// `hasEqualProperties`. Both `predicate` reads bind the instance field —
/// the oracle is silent (probe2482 c: `No issues`).
#[test]
fn field_shadow_in_comparison_stays_silent() {
    let source = "\
import 'package:t/meta.dart';

@useResult
Predicate predicate(int length, Predicate predicate) =>
    Predicate(length, predicate);

class Predicate {
  Predicate(this.length, this.predicate);

  final int length;
  final Predicate predicate;

  bool hasEqualProperties(Predicate other) =>
      length == other.length && predicate == other.predicate;
}
";
    assert!(unused_result_rows(source).is_empty(), "{source:?}");
}

/// The local-binding face of the same rule: a local variable of the
/// annotated declaration's spelling owns the read (the lexical scope beats
/// both the library declaration and any member) — the oracle is silent
/// (probe2482 d: `No issues`).
#[test]
fn local_variable_shadow_stays_silent() {
    let source = "\
import 'package:t/meta.dart';

@useResult
int tally() => 1;

void run() {
  final tally = 3;
  tally;
}
";
    assert!(unused_result_rows(source).is_empty(), "{source:?}");
}

/// The keep-honest twin of the `Opaque` resolution face: an INHERITED
/// object member shares its spelling with a library declaration but loses
/// to it upstream (`int f = 2; ... f` binds the field, while `hashCode`
/// binds the top-level function — probe2482 e: `unused_result` at the
/// read), so the resolution gate must not silence this shape.
#[test]
fn inherited_object_member_shadow_still_reports() {
    let source = "\
import 'package:t/meta.dart';

@useResult
double hashCode() => 1.5;

class C {
  void m() {
    hashCode;
  }
}
";
    let at = source.find("\n    hashCode;").unwrap() as u32 + 5;
    assert_eq!(
        unused_result_rows(source),
        vec![(at, "hashCode".len() as u32)],
        "{source:?}"
    );
}

/// The #2478 honesty pin restated inside this lane's rig: a GENUINE
/// discarded `@useResult` top-level invocation keeps reporting at the
/// callee name token (probe2482 f: one row). The resolution gate must not
/// touch the declaration-bound face.
#[test]
fn genuine_discarded_use_result_still_fires() {
    let source = "\
import 'package:t/meta.dart';

@useResult
int predicate(int x) => x;

void fire() {
  predicate(1);
}
";
    let at = source.rfind("predicate").unwrap() as u32;
    assert_eq!(
        unused_result_rows(source),
        vec![(at, "predicate".len() as u32)],
        "{source:?}"
    );
}
