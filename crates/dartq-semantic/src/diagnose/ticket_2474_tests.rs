//! The #2474 pins: bare names inside an `extension` body resolve against
//! the `on` type's member face even when the `on` head's clause walk
//! cannot reach it (census G1, x3_extthis + the w-drift `listen`/
//! `isBroadcast`/`open`/`session`/`streamQueries`/`map` rows on Dart
//! 3.13.1).
//!
//! `this` inside `extension E<S> on Stream<S>` types as `Stream<S>`; a
//! bare `listen(...)` or `isBroadcast` resolves through the extended
//! type's interface (the oracle's `ExtensionMemberScope`). The clause
//! walk's `resolve_clause_face` on an SDK head answers `Missing`/`Unknown`
//! — `Stream` spells ambiguously across the `dart:core`/`dart:async`
//! synthetic face files (two `Decl` entries), so `walk_face_list`
//! classifies the hop dead — and the bare miss surfaced as
//! `undefined_method`/`undefined_identifier` for 'Stream'. The on-face
//! walk now consults the frozen member surface before letting a
//! non-`Found` verdict stand: `Defined` suppresses exactly like a broken
//! chain (the member lives on the `on` type), `Absent` keeps the walk's
//! clean miss (a name `Stream` genuinely lacks still reports), `Unknown`
//! stays undecidable.

use super::*;
use crate::collector::collect;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_resolution::LibraryGraphBuilder;
use dartq_scanner::ScanConfig;
use dartq_source::FileTable;
use std::path::Path;

/// Diagnoses `source` as the sole file of a library with no imports —
/// the single-file rig (`ticket_2245_tests` carries the same shape).
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

fn code_names(report: &FileReport) -> Vec<&'static str> {
    report
        .diagnostics
        .iter()
        .map(|d| d.code.name)
        .collect::<Vec<_>>()
}

/// The frozen-face arm: `on Stream<S>` in a file that cannot resolve
/// `Stream` (no SDK face in the rig) — the frozen `Stream` member table
/// still declares `listen`, so the bare invocation suppresses (the
/// drift corpus's `async_map.dart:29` row direction; the oracle's real
/// `Stream` resolves and carries `listen`).
#[test]
fn bare_invoke_on_unresolvable_stream_on_type_stays_silent() {
    let source = "\
extension E<S> on Stream<S> {
  void m() {
    listen((data) {});
  }
}
";
    let names = code_names(&report(source));
    for code in ["undefined_method", "undefined_identifier"] {
        assert!(!names.contains(&code), "{code} -> {names:?}");
    }
}

/// The getter spelling on the same unresolvable `Stream` on-type —
/// `isBroadcast` is a frozen `Stream` member, so the bare read
/// suppresses (the x3 probe's `isBroadcast: isBroadcast` argument).
#[test]
fn bare_getter_on_unresolvable_stream_on_type_stays_silent() {
    let source = "\
extension E<S> on Stream<S> {
  void m() {
    var b = isBroadcast;
    print(b);
  }
}
void print(Object? o) {}
";
    let names = code_names(&report(source));
    assert!(!names.contains(&"undefined_identifier"), "{names:?}");
}

/// The keep-reporting guard: `Stream`'s frozen surface does NOT declare
/// `foo`, so `Absent` keeps the walk's miss — a resolved local `Stream`
/// reports `undefined_method` for 'Stream' at the bare invocation.
#[test]
fn absent_member_on_resolved_on_type_still_reports() {
    let source = "\
class Stream<S> {
  void listen(void Function(S) f) {}
}

extension E<S> on Stream<S> {
  void m() {
    foo();
  }
}
";
    let names = code_names(&report(source));
    assert!(names.contains(&"undefined_method"), "{names:?}");
}

/// The local `Stream` `Found` path: `listen` resolves through the
/// container climb directly — the frozen consult never fires and nothing
/// reports.
#[test]
fn resolved_local_on_type_member_is_silent() {
    let source = "\
class Stream<S> {
  void listen(void Function(S) f) {}
}

extension E<S> on Stream<S> {
  void m() {
    listen((data) {});
  }
}
";
    let names = code_names(&report(source));
    assert!(!names.contains(&"undefined_method"), "{names:?}");
}

/// The type-parameter on-type shape: `on List<T>` with an unresolvable
/// `List` — `add` is a frozen `List` member, so the bare invocation
/// suppresses exactly like the `Stream` face.
#[test]
fn bare_invoke_on_unresolvable_list_on_type_stays_silent() {
    let source = "\
extension E<T> on List<T> {
  void m(T v) {
    add(v);
  }
}
";
    let names = code_names(&report(source));
    assert!(!names.contains(&"undefined_method"), "{names:?}");
}
