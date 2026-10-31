//! The #3132 pins: `body_might_complete_normally`'s async-union verdict.
//!
//! Upstream citations:
//! - `checkForBodyMayCompleteNormally` reads the async body's completion
//!   context — `_contextTypeForImposed`'s final arm synthesizes
//!   `FutureOr<futureValueType(S)>` (upstream Dart SDK source,
//!   pkg/analyzer/lib/src/generated/resolver.dart:538-611 and
//!   pkg/analyzer/lib/src/dart/resolver/body_inference_context.dart's
//!   `_contextTypeForImposed`; `TypeSystem.futureValueType` at
//!   pkg/analyzer/lib/src/dart/element/type_system.dart:471).
//! - The synthesized `FutureOr` head is materialized by the type system —
//!   never through the user scope's import table — so `Future<int> f()
//!   async {}` reports `'FutureOr<int>'` even with no `dart:async`
//!   import, where a WRITTEN unimported `FutureOr` is `InvalidType` and
//!   stays silent (upstream Dart SDK source,
//!   pkg/analyzer/lib/src/generated/resolver.dart's
//!   `_checkForFutureCatchErrorOnError`/`isPotentiallyNonNullable`
//!   twin-selection).
//! - `FutureOr<T>` is nullable exactly when `T` is
//!   (`typeSystem.isNullable` delegates to the argument), so
//!   `FutureOr<int?>`/`FutureOr<Object?>` reach the NULLABLE twin.

use super::*;
use crate::collector::collect;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_resolution::LibraryGraphBuilder;
use dartq_scanner::ScanConfig;
use dartq_source::FileTable;
use std::path::Path;

/// Diagnoses `source` as the sole file of a library with no imports —
/// the single-file rig (`ticket_3109_tests`' shape).
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

fn codes(report: &FileReport) -> Vec<&'static str> {
    report.diagnostics.iter().map(|d| d.code.name).collect()
}

/// `Future<int> f() async {}` under a `FutureOr`-less scope still reports
/// the ERROR twin — the synthesized `FutureOr<int>` union's head is
/// materialized by the type system, never gated on the user scope's
/// `dart:async` import (resolver.dart:573-603's
/// `isPotentiallyNonNullable` pick on the `FutureOr<V>` context).
#[test]
fn async_future_int_reports_error_without_futureor_in_scope() {
    let report = report("class int {} class Future<T> {} Future<int> f() async {}");
    let rows: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "body_might_complete_normally")
        .collect();
    assert_eq!(rows.len(), 1, "{:?}", codes(&report));
    assert_eq!(
        rows[0].message,
        "The body might complete normally, causing 'null' to be returned, but the return type, 'FutureOr<int>', is a potentially non-nullable type."
    );
}

/// `FutureOr<int>` written WITHOUT importing `dart:async`, in an async
/// body, reports the NULLABLE twin with `FutureOr<Object?>` — the
/// unimported `FutureOr` head is `InvalidType` upstream, and
/// `futureValueType(InvalidType)` answers `Object?`.
#[test]
fn async_unimported_futureor_reports_nullable_object() {
    let report = report("class int {} FutureOr<int> f() async {}");
    let rows: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "body_might_complete_normally_nullable")
        .collect();
    assert_eq!(rows.len(), 1, "{:?}", codes(&report));
    assert_eq!(
        rows[0].message,
        "This function has a nullable return type of 'FutureOr<Object?>', but ends without returning a value."
    );
    assert!(
        !codes(&report).contains(&"body_might_complete_normally"),
        "{:?}",
        codes(&report)
    );
}

/// `Future<int?> f() async {}` reports the NULLABLE twin — the union's
/// nullability is the future value type's own (`FutureOr<int?>` is
/// nullable because `int?` is).
#[test]
fn async_future_int_nullable_reports_nullable() {
    let report =
        report("class int {} class Future<T> {} class FutureOr<T> {} Future<int?> f() async {}");
    let rows: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "body_might_complete_normally_nullable")
        .collect();
    assert_eq!(rows.len(), 1, "{:?}", codes(&report));
    assert_eq!(
        rows[0].message,
        "This function has a nullable return type of 'FutureOr<int?>', but ends without returning a value."
    );
}

/// A WRITTEN `FutureOr<int>` in a SYNC body without `dart:async` stays
/// silent — `InvalidType` upstream asks no nullability question (the
/// `named_type_is_type` gate that the synthesized union bypasses still
/// applies to the written head).
#[test]
fn sync_unimported_futureor_stays_silent() {
    let report = report("class int {} FutureOr<int> f() {}");
    assert!(
        !codes(&report).contains(&"body_might_complete_normally"),
        "{:?}",
        codes(&report)
    );
}

/// A `void`-valued async union (`Future<void>`, `Future`, `dynamic`)
/// stays silent — the `FutureOr<void>`/`dynamic` base admits a normal
/// completion (resolver.dart:592-602's silent-base arm).
#[test]
fn async_void_union_stays_silent() {
    let report = report(
        "class Future<T> {} Future<void> f() async {} \
         Future g() async {} dynamic h() async {}",
    );
    assert!(
        !codes(&report).contains(&"body_might_complete_normally"),
        "{:?}",
        codes(&report)
    );
    assert!(
        !codes(&report).contains(&"body_might_complete_normally_nullable"),
        "{:?}",
        codes(&report)
    );
}
