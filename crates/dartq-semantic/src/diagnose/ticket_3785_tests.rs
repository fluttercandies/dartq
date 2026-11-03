//! The #3785 pins: object-pattern field unions and `&&` head/object
//! conjunctions over a sealed split, probed against oracle `dart
//! analyze` on an isolated tree before pinning.
//!
//! - `King<int>(suit: Suit.club || Suit.diamond || Suit.heart ||
//!   Suit.spade)` drains its field as a `Union` leaf — the row keeps the
//!   property sub-space instead of degrading whole-pattern, so the
//!   four-constant union covers the `suit` column
//!   (co19 `exhaustiveness_object_A01_t01`/`_t02`/`_t05`/`_t06`).
//! - `Jack<num>() && Jack<T>(oneEyed: _)` intersects the bare type test
//!   with the object row on the narrower head, covering `Jack<T>` over
//!   `Face<T>` and `FaceET<T>` scrutinees
//!   (`exhaustiveness_object_A01_t03`/`_t04`).
//!
//! The same shapes on a plain (non-extension-type) scrutinee pin the
//! fix below — the extension-type spellings ride the identical
//! restriction faces through `NameWorld::extension_erasure`.

use super::*;
use crate::collector::collect;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_resolution::{
    Combinators, ImportEdge, LibraryClassModifiers, LibraryGraphBuilder, Namespaces,
};
use dartq_scanner::ScanConfig;
use dartq_source::FileTable;
use std::path::Path;

/// One-file rig (mirrors `tests.rs::report` plus the ticket_2498 SDK
/// face): the live fold pipeline over a single library, with the
/// dart:core primitives the fixtures name (`num`/`int`/`String`/`bool`/
/// `double`/`Object`) declared on an unindexed `<dart:core>` node joined
/// by an unfiltered unprefixed import.
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
    let core = builder.intern("dart:core");
    let core_file = files.intern(Path::new("<dart:core>"));
    for name in [
        "num", "int", "double", "String", "bool", "Object", "Null", "Enum", "Record",
        "List", "Map", "Set", "Iterable", "Future", "Stream",
    ] {
        builder.declare(core, strings.intern(name), core_file);
    }
    builder.import(
        lib,
        ImportEdge {
            target: Some(core),
            prefix: None,
            deferred: false,
            non_library: false,
            combinators: Combinators::unfiltered(),
        },
    );
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
    report.diagnostics.iter().map(|d| d.code.name).collect()
}

/// The shared sealed hierarchy the pins switch over (co19's
/// `exhaustiveness_lib.dart` shape, verbatim members).
const DECK: &str = "\
enum Suit { club, diamond, heart, spade }
sealed class Card<T> { final Suit suit; Card(this.suit); }
sealed class Face<T> extends Card<T> { Face(super.suit); }
class Jack<T> extends Face<T> { final bool oneEyed; Jack(super.suit, {this.oneEyed = false}); }
class Queen<T> extends Face<T> { Queen(super.suit); }
class King<T> extends Face<T> { King(super.suit); }
extension type FaceET1<T>(Face<T> _) {}
";

/// `King<int>(suit: club || diamond || heart || spade)` — the field's
/// `||` sub-pattern unions the alternatives over the column, so the
/// four constants cover `suit` whole. Oracle: silent (exhaustive).
#[test]
fn object_field_enum_union_covers_column() {
    let source = format!(
        "{DECK}\
String f(Face<int> face) => switch (face) {{
  Jack<num>() => 'J',
  Queen<int>() => 'Q',
  King<int>(suit: Suit.club || Suit.diamond || Suit.heart || Suit.spade) => 'K',
}};"
    );
    let report = report(&source);
    assert_eq!(report.diagnostics.len(), 0, "{:?}", code_names(&report));
}

/// The same union over the extension-type scrutinee — the erased
/// `Face<int>` split meets the un-erased `King<int>` case heads the
/// same way. Oracle: silent.
#[test]
fn extension_type_scrutinee_object_field_union_covers() {
    let source = format!(
        "{DECK}\
String f(FaceET1<int> face) => switch (face) {{
  Jack<num>() => 'J',
  Queen<int>() => 'Q',
  King<int>(suit: Suit.club || Suit.diamond || Suit.heart || Suit.spade) => 'K',
}};"
    );
    let report = report(&source);
    assert_eq!(report.diagnostics.len(), 0, "{:?}", code_names(&report));
}

/// A three-constant union still leaves `spade` uncovered — the oracle
/// reports `King<int>(suit: Suit.spade)` verbatim.
#[test]
fn object_field_enum_union_missing_constant_reports_witness() {
    let source = format!(
        "{DECK}\
String f(Face<int> face) => switch (face) {{
  Jack<num>() => 'J',
  Queen<int>() => 'Q',
  King<int>(suit: Suit.club || Suit.diamond || Suit.heart) => 'K',
}};"
    );
    let report = report(&source);
    assert_eq!(
        code_names(&report),
        ["non_exhaustive_switch_expression"],
        "{:?}",
        code_names(&report)
    );
    assert_eq!(
        report.diagnostics[0].message,
        "The type 'Face<int>' isn't exhaustively matched by the switch cases since it doesn't match the pattern 'King<int>(suit: Suit.spade)'."
    );
}

/// `Jack<num>() && Jack<T>(oneEyed: _)` over `Face<T>` — the `&&`
/// narrows the object row's head to `Jack`, covering `Jack<T>` whole.
/// Oracle: silent (co19 `exhaustiveness_object_A01_t01`'s `test3_1`
/// shape).
#[test]
fn and_object_head_refinement_covers_parametric_leaf() {
    let source = format!(
        "{DECK}\
String f<T extends num>(Face<T> face) => switch (face) {{
  Jack<num>() && Jack<T>(oneEyed: _) => 'J',
  Queen<num>() => 'Q',
  King<T>() => 'K',
}};"
    );
    let report = report(&source);
    assert_eq!(report.diagnostics.len(), 0, "{:?}", code_names(&report));
}

/// The same `&&` refinement through the extension-type face —
/// `FaceET1<T>` erases to `Face<T>` for the split while the case rows
/// keep their own heads. Oracle: silent.
#[test]
fn extension_type_scrutinee_and_object_refinement_covers() {
    let source = format!(
        "{DECK}\
String f<T extends num>(FaceET1<T> face) => switch (face) {{
  Jack<num>() && Jack<T>(oneEyed: _) => 'J',
  Queen<num>() => 'Q',
  King<T>() => 'K',
}};"
    );
    let report = report(&source);
    assert_eq!(report.diagnostics.len(), 0, "{:?}", code_names(&report));
}
