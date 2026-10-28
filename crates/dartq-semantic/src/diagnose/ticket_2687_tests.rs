//! The #2687 creation-inference pins: the last corpus differential row
//! (`riverpod state_provider.dart 116:47`, `argument_type_not_assignable`)
//! burned down to four stacked faces, each pinned here against the oracle
//! (`dart analyze` 3.47.1, /tmp fixtures):
//!
//! - A class whose `extends` head resolves nowhere leaves its super
//!   formals untypeable, so a generic creation over it can never gain an
//!   upward candidate: the oracle's unsolved fill (the bound, else
//!   `dynamic`) is the exact instantiation (`StateController<dynamic>`).
//! - The `=` the send is the RHS of offers the write target's static type
//!   as the send's contextType (`assignment_expression_resolver.dart`):
//!   the factory's parameter solves DOWNWARD from it
//!   (`$Result.data(controller)` against `$Result<StateController<ValueT>>?`).
//! - A redirecting factory's own written formals are the invocation's
//!   checked parameters (`factory $Result.data(ValueT state) =
//!   $ResultData;`).
//! - An in-scope type parameter is a real type, never generic-inference
//!   residue: `C<dynamic>` does not fit `C<E>` (`dynamic` is not a
//!   subtype of the variable).
//!
//! The near-misses pin the boundaries the oracle keeps silent: a
//! resolvable superclass constrains the creation normally, a bare
//! factory call without the write context solves upward, and a
//! declared-type context on the creation itself matches the parameter.

use super::*;
use crate::collector::collect;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_resolution::{LibraryClassModifiers, LibraryGraphBuilder};
use dartq_scanner::ScanConfig;
use dartq_source::FileTable;
use std::path::Path;

fn scan(source: &str, file: FileId) -> (dartq_scanner::ScanResult, ParserFeatures, FeatureSet) {
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
    (scanned, features, set)
}

/// Diagnoses `source` as the sole file of a library with no imports (the
/// `ticket_undefined_family_tests` rig mirrored).
fn report(source: &str) -> FileReport {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
    let (scanned, features, set) = scan(source, file);
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

/// The corpus shape: an unresolved superclass leaves the creation's type
/// parameter unsolved (filled `dynamic`), the assignment's write context
/// solves the factory's parameter to the enclosing class's type
/// parameter, and the argument check reports — the oracle's exact
/// message face (`Ctrl<dynamic>` into `Ctrl<E>`).
#[test]
fn unresolved_super_creation_fills_and_write_context_types_factory_param() {
    let report = report(
        "class Ctrl<T> extends StateNotifier<T> {
  Ctrl(super.state);
}
class Res<V> {
  factory Res.data(V v) = ResData;
}
final class ResData<V> implements Res<V> {
  ResData(this.value);
  final V value;
}
class Obs<V> {
  Res<V>? result;
}
class Holder<E> {
  final obs = Obs<Ctrl<E>>();
  E makeInit() => throw '';
  void create() {
    final init = makeInit();
    final controller = Ctrl(init);
    obs.result = Res.data(controller);
  }
}",
    );
    let row = report
        .diagnostics
        .iter()
        .find(|d| d.code.name == "argument_type_not_assignable")
        .expect("the write-context parameter mismatch reports");
    assert!(
        row.message.contains("'Ctrl<dynamic>'"),
            "the creation fill lands: {}",
            row.message
    );
    assert!(
        row.message.contains("'Ctrl<E>'"),
            "the write context solved the factory parameter: {}",
            row.message
    );
}

/// A resolvable superclass constrains the creation normally (the super
/// formal types the parameter): no fill, no mismatch.
#[test]
fn resolvable_super_creation_stays_silent() {
    let report = report(
        "class Notifier<T> {
  Notifier(T state);
}
class Ctrl<T> extends Notifier<T> {
  Ctrl(super.state);
}
class Res<V> {
  factory Res.data(V v) = ResData;
}
final class ResData<V> implements Res<V> {
  ResData(this.value);
  final V value;
}
class Obs<V> {
  Res<V>? result;
}
class Holder<E> {
  final obs = Obs<Ctrl<E>>();
  E makeInit() => throw '';
  void create() {
    final init = makeInit();
    final controller = Ctrl(init);
    obs.result = Res.data(controller);
  }
}",
    );
    assert!(
        !report
            .diagnostics
            .iter()
            .any(|d| d.code.name == "argument_type_not_assignable"),
        "the constrained creation matches the context-solved parameter"
    );
}

/// Without the assignment (`Res.data(controller);` as a bare statement)
/// the factory's parameter solves upward from the argument: matching
/// types, silent.
#[test]
fn bare_factory_call_without_write_context_stays_silent() {
    let report = report(
        "class Ctrl<T> extends StateNotifier<T> {
  Ctrl(super.state);
}
class Res<V> {
  factory Res.data(V v) = ResData;
}
final class ResData<V> implements Res<V> {
  ResData(this.value);
  final V value;
}
class Holder<E> {
  E makeInit() => throw '';
  void create() {
    final controller = Ctrl(makeInit());
    Res.data(controller);
  }
}",
    );
    assert!(
        !report
            .diagnostics
            .iter()
            .any(|d| d.code.name == "argument_type_not_assignable"),
        "the upward solve matches the filled argument"
    );
}

/// A declared-type context on the creation itself solves its parameter
/// (`Ctrl<E> controller = Ctrl(init);` — the official downward face):
/// the local matches the write-context-solved parameter, silent.
#[test]
fn declared_context_on_creation_matches_and_stays_silent() {
    let report = report(
        "class Ctrl<T> extends StateNotifier<T> {
  Ctrl(super.state);
}
class Res<V> {
  factory Res.data(V v) = ResData;
}
final class ResData<V> implements Res<V> {
  ResData(this.value);
  final V value;
}
class Obs<V> {
  Res<V>? result;
}
class Holder<E> {
  final obs = Obs<Ctrl<E>>();
  E makeInit() => throw '';
  void create() {
    final init = makeInit();
    Ctrl<E> controller = Ctrl(init);
    obs.result = Res.data(controller);
  }
}",
    );
    assert!(
        !report
            .diagnostics
            .iter()
            .any(|d| d.code.name == "argument_type_not_assignable"),
        "the declared-context solve matches the write context"
    );
}
