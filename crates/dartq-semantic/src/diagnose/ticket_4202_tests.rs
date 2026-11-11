//! #4202 pins: the executable type-parameter windows are scanned in
//! DECLARATION order, never hash order.
//!
//! Two top-level (or member) generics in one file may share a parameter
//! spelling with different bounds — `f<T extends Base>` next to `g<T>`.
//! The post-walk bound lookups used to scan `FxHashMap<StringId, _>`
//! captures, so the window a reference resolved through depended on the
//! map's iteration order, which moves between runs because parallel
//! scanning interns names in a scheduler-dependent order: one run read the
//! bounded window, the next the unbounded (`Object?`) one, and the
//! receiver-nullability verdict flipped with it. The lookups now read one
//! declaration-ordered list and narrow to the nearest declaring window at
//! or before the reference, so each reference sees its own scope.

use crate::collector::collect;
use crate::diagnose::{FileReport, StrictModes, diagnose_with};
use crate::library::ElementIndex;
use crate::model::MicroKind;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_resolution::{LibraryClassModifiers, LibraryGraphBuilder, Namespaces};
use dartq_scanner::ScanConfig;
use dartq_source::{FileTable, StringTable};
use std::path::Path;

/// The one-library rig: `source` is the sole analyzed file of
/// `package:t`, no imports.
fn unit_report(source: &str) -> FileReport {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
    let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
    let features = ParserFeatures::from_feature_set(set);
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
    let elements = collect(source, &scanned.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib = builder.intern("package:t/main.dart");
    for (name, decl) in elements.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(lib, name, file);
    }
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    let mut index = ElementIndex::default();
    index.insert(file, elements);
    diagnose_with(
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
    )
}

fn count_code(report: &FileReport, name: &str) -> usize {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == name)
        .count()
}

fn at(source: &str, needle: &str) -> u32 {
    at_nth(source, needle, 0)
}

/// The `nth` (0-based) occurrence of `needle`, an offset.
fn at_nth(source: &str, needle: &str, nth: usize) -> u32 {
    let index = source
        .match_indices(needle)
        .nth(nth)
        .map(|(index, _)| index)
        .unwrap_or_else(|| panic!("{needle:?} #{nth} not in {source:?}"));
    u32::try_from(index).expect("offset fits u32")
}

/// The selector offset (`count`) of the `nth` `value.count` read — member
/// rows anchor at the selector token, not the receiver.
fn member_at(source: &str, nth: usize) -> u32 {
    at_nth(source, "value.count", nth) + u32::try_from("value.".len()).expect("fits")
}

/// Exactly one row of `name`, anchored at `offset`.
fn one_at(report: &FileReport, name: &str, offset: u32, source: &str) {
    let rows: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == name)
        .collect();
    assert_eq!(rows.len(), 1, "{name}: {rows:?}");
    assert_eq!(rows[0].range.offset, offset, "in {source:?}");
}

/// The corpus witness shape (`_uniqueEnumSet<T extends Enum>` beside an
/// unbounded `_sameSet<T>`): the bounded function's member read keeps its
/// own bound no matter that an unbounded twin exists later in the file —
/// no `unchecked_use_of_nullable_value` row.
#[test]
fn bounded_twin_declared_first_keeps_its_bound() {
    let source = "\
class Base {
  int count = 0;
}

int readBounded<T extends Base>(T value) => value.count;

int readUnbounded<T>(T value) => value.count;
";
    let report = unit_report(source);
    // Only the unbounded twin owes the row; the bounded one reads `Base`.
    one_at(
        &report,
        "unchecked_use_of_nullable_value",
        member_at(source, 1),
        source,
    );
}

/// The reverse declaration order: the bounded twin's reference still
/// resolves through its OWN window (the nearest declaring window at or
/// before the reference), not the first-declared unbounded one.
#[test]
fn bounded_twin_declared_second_still_keeps_its_bound() {
    let source = "\
class Base {
  int count = 0;
}

int readUnbounded<T>(T value) => value.count;

int readBounded<T extends Base>(T value) => value.count;
";
    let report = unit_report(source);
    let first = member_at(source, 0);
    let second = member_at(source, 1);
    one_at(&report, "unchecked_use_of_nullable_value", first, source);
    // The bounded twin's read stays clean.
    assert!(
        report
            .diagnostics
            .iter()
            .all(|d| d.code.name != "unchecked_use_of_nullable_value" || d.range.offset != second),
        "bounded twin must not inherit the unbounded window: {source:?}"
    );
}

/// A `?`-suffixed bound keeps the nullable receiver face: the row still
/// fires exactly once (the fix narrows windows, it never suppresses the
/// genuinely nullable verdict).
#[test]
fn nullable_bound_twin_keeps_the_row() {
    let source = "\
class Base {
  int count = 0;
}

int readNullable<T extends Base?>(T value) => value.count;
";
    let report = unit_report(source);
    one_at(
        &report,
        "unchecked_use_of_nullable_value",
        member_at(source, 0),
        source,
    );
}

/// The member-header flavor of the same shape: two methods of one class,
/// bounded first, unbounded second — the unbounded one reports, the
/// bounded one stays silent.
#[test]
fn member_twin_bounds_resolve_per_method() {
    let source = "\
class Base {
  int count = 0;
}

class Box {
  int readBounded<T extends Base>(T value) => value.count;
  int readUnbounded<T>(T value) => value.count;
}
";
    let report = unit_report(source);
    one_at(
        &report,
        "unchecked_use_of_nullable_value",
        member_at(source, 1),
        source,
    );
}
