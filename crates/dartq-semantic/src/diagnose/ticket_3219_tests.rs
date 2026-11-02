//! #3219 pins — F9-B: a class declaration in a library that PREDATES the
//! `class-modifiers` feature carries no modifier facts.
//!
//! Upstream gates the parser on the declaring file's feature set
//! (`enableClassModifiers = _featureSet.isEnabled(Feature.class_modifiers)`,
//! upstream Dart SDK source, `pkg/analyzer/lib/src/fasta/ast_builder.dart`):
//! a `base`/`final`/`interface`/`sealed`/`mixin class` written in such a
//! library reports `experiment_not_enabled` and never reaches the element
//! model. The restriction families therefore stay silent for a pre-feature
//! declaration, whatever library consumes it — the shape the oracle answers
//! for the co19 `Class-modifiers/versioning_A02_*` suites.
//!
//! dartq records the modifier flags straight from the syntax, so before this
//! ticket a pre-feature declaration still looked `base`/`final` to every
//! consumer: `invalid_use_of_type_outside_library` (extends/implements/`on`
//! outside a library) and `subtype_of_base_or_final_is_not_base_final_or_sealed`
//! rows fired where the oracle emits none (71 EXTRA rows on the entry):
//! `FoldHierarchy::declared` now masks the flag set to none when the
//! DECLARING library's `LibraryClassModifiers` posture says the feature is
//! off (`enabledIn 3.0.0`), which every modifier consumer reads through
//! `effective`/`ModifierHierarchy::modifiers`.
//!
//! The rig below is local to this pin file (the shared `test_rig` carries a
//! single analyzed unit and cannot express a per-library posture): each
//! fixture library is scanned and collected at its own `// @dart` version,
//! and the posture map mirrors what the engine's `build_class_modifiers`
//! derives from the same feature set.
//!
//! Registered through `diagnose/tests.rs`'s `#[path]` block: the brief's
//! named files are live lanes out of this ticket's write scope, and `#[path]`
//! keeps the module inside the `diagnose` subtree so `crate::diagnose::*`
//! resolves.

use crate::collector::collect;
use crate::diagnose::test_rig::rows;
use crate::diagnose::{FileReport, StrictModes, fold_unit_with_ops, merge_source_ordered};
use crate::library::ElementIndex;
use crate::model::MicroKind;
use dartq_language::{Feature, FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_resolution::{
    ClassModifiers, Combinators, ImportEdge, LibraryClassModifiers, LibraryGraphBuilder, Namespaces,
};
use dartq_scanner::ScanConfig;
use dartq_source::{FileTable, StringTable};
use std::path::Path;

/// The names-only `dart:core` stub the rig declares, so the special
/// spellings a fixture may name resolve (the shared rig's list verbatim).
const CORE_STUB_NAMES: [&str; 10] = [
    "Object", "Never", "Null", "Function", "Future", "FutureOr", "Record", "Enum", "Type", "Symbol",
];

/// The two codes of the `class_modifiers` restriction families: the
/// outside-library rows (`extends`/`implements`/`with`/`on` outside the
/// declaring library) and the must-be-base/final/sealed subtype rows.
const MODIFIER_CODES: [&str; 2] = [
    "invalid_use_of_type_outside_library",
    "subtype_of_base_or_final_is_not_base_final_or_sealed",
];

/// One fixture library: its `package:` URI and source. Its language version
/// comes from the corpus `// @dart=X.Y` marker (the third line at the
/// latest), the same reading the shared rig uses.
struct RigLib<'a> {
    uri: &'a str,
    source: &'a str,
}

/// The fixture's own language version: a leading `// @dart=X.Y` marker pins
/// it, everything else analyzes at the latest.
fn language_version(source: &str) -> LanguageVersion {
    for line in source.lines().take(3) {
        let line = line.trim();
        if !line.is_empty() && !line.starts_with("//") {
            break;
        }
        if let Some(rest) = line.strip_prefix("// @dart=")
            && let Some((major, minor)) = rest.trim().split_once('.')
            && let (Ok(major), Ok(minor)) = (major.parse(), minor.parse())
        {
            return LanguageVersion::new(major, minor);
        }
    }
    LanguageVersion::new(3, 15)
}

fn scan_fixture(source: &str, file: dartq_source::FileId) -> dartq_scanner::ScanResult {
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
}

/// Diagnoses every fixture library: `libs[0]` is the main unit, the rest are
/// imported by it. Answers one report per library, in fixture order.
fn analyze(libs: &[RigLib<'_>]) -> Vec<FileReport> {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let mut builder = LibraryGraphBuilder::new(&strings);
    let core_lib = builder.intern("dart:core");
    let core_file = files.intern(Path::new("<dart:core>"));
    for name in CORE_STUB_NAMES {
        builder.declare(core_lib, strings.intern(name), core_file);
    }
    let mut index = ElementIndex::default();
    let mut posture = LibraryClassModifiers::default();
    let mut units = Vec::new();
    for lib in libs {
        let version = language_version(lib.source);
        let features = ParserFeatures::from_feature_set(FeatureSet::for_language_version(version));
        let file = files.intern(Path::new(lib.uri));
        let scanned = scan_fixture(lib.source, file);
        let elements = collect(lib.source, &scanned.tokens, features, &strings);
        let lib_id = builder.intern(lib.uri);
        builder.unit(lib_id, file);
        for (name, decl) in elements.declarations() {
            if decl.kind == MicroKind::TypeParam {
                continue;
            }
            if matches!(
                decl.kind,
                MicroKind::Class | MicroKind::Enum | MicroKind::Mixin
            ) {
                builder.declare_class(lib_id, name, file, ClassModifiers::default());
            } else {
                builder.declare(lib_id, name, file);
            }
        }
        builder.import(
            lib_id,
            ImportEdge {
                target: Some(core_lib),
                prefix: None,
                deferred: false,
                non_library: false,
                combinators: Combinators::unfiltered(),
            },
        );
        index.insert(file, elements);
        // The engine's `build_class_modifiers` posture: the defining file's
        // settled version through the feature registry.
        let class_modifiers = FeatureSet::for_language_version(version).is_enabled(Feature::ClassModifiers);
        posture.insert(lib_id, class_modifiers);
        units.push((lib_id, file, scanned, features, lib.source));
    }
    // The main unit imports every sibling, in fixture order.
    for (lib_id, ..) in &units[1..] {
        builder.import(
            units[0].0,
            ImportEdge {
                target: Some(*lib_id),
                prefix: None,
                deferred: false,
                non_library: false,
                combinators: Combinators::unfiltered(),
            },
        );
    }
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    units
        .iter()
        .map(|(lib_id, file, scanned, features, source)| {
            let fold = fold_unit_with_ops(
                source,
                &scanned.tokens,
                *features,
                &strings,
                *file,
                *lib_id,
                &graph,
                &namespaces,
                &index,
                StrictModes::default(),
                &posture,
                true,
                None,
            );
            let mut report = fold.report;
            report.diagnostics = merge_source_ordered(fold.parse_diagnostics, report.diagnostics);
            report
        })
        .collect()
}

/// The rows of the two `class_modifiers` restriction families.
fn modifier_rows(report: &FileReport) -> usize {
    report
        .diagnostics
        .iter()
        .filter(|d| MODIFIER_CODES.contains(&d.code.name))
        .count()
}

/// The EXTRA→fixed shape: a pre-feature library (`// @dart=2.19`) declares
/// `base`/`final` classes; a post-feature consumer extends/implements them
/// across the library boundary. Upstream sees no modifier on the declaring
/// elements, so BOTH restriction families stay silent — the exact
/// `versioning_A02_t02`/`_t03` shape (pre-fix: one
/// `invalid_use_of_type_outside_library` per `implements` face plus the
/// subtype row per extending subtype).
#[test]
fn pre_feature_declaring_library_masks_modifier_facts() {
    let dep = "\
// @dart=2.19
base class BaseClass {}
final class FinalClass {}
base mixin class BaseMixinClass {}
";
    let main = "\
import 'package:t/db.dart';

class ImplementsBase implements BaseClass {}
class ExtendsFinal extends FinalClass {}
class ImplementsBaseMixin implements BaseMixinClass {}
mixin OnBase on BaseClass {}
";
    let reports = analyze(&[
        RigLib {
            uri: "package:t/main.dart",
            source: main,
        },
        RigLib {
            uri: "package:t/db.dart",
            source: dep,
        },
    ]);
    assert_eq!(
        modifier_rows(&reports[0]),
        0,
        "a pre-feature declaring library carries no modifier facts, so no \
         restriction row may fire: {}",
        rows(&reports[0]).join("\n")
    );
}

/// The mask is version-gated, not blanket-off: the same declarations in a
/// `// @dart=3.0` library DO restrict their out-of-library consumers.
#[test]
fn post_feature_declaring_library_keeps_modifier_restrictions() {
    let dep = "\
// @dart=3.0
final class FinalClass {}
";
    let main = "\
import 'package:t/db.dart';

class ExtendsFinal extends FinalClass {}
";
    let reports = analyze(&[
        RigLib {
            uri: "package:t/main.dart",
            source: main,
        },
        RigLib {
            uri: "package:t/db.dart",
            source: dep,
        },
    ]);
    assert_eq!(
        modifier_rows(&reports[0]),
        1,
        "a `final` class declared at 3.0 is not extendable outside its \
         library: {}",
        rows(&reports[0]).join("\n")
    );
}

/// The `versioning_A02_t03` shape end to end: the entry library is itself a
/// pre-feature library (`// @dart=2.19`) importing a sibling that inherits the
/// same version from the case package. The outside-library family stays
/// silent for the sibling's `base`/`final` declarations — the 71 EXTRA rows
/// of the corpus census are exactly these faces.
///
/// The `experiment_not_enabled` rows the oracle reports on the declaring unit
/// are not asserted here: they are produced by the language-version layer, not
/// by this seam — a semantic fold's `parse_diagnostics` carry only the parser's
/// experimental flags (triple-shift, patterns, variance, augmentations,
/// primary constructors), and the class-modifier gate is not one of them. The
/// mask below reads the modifier FACTS only, so it cannot touch that face.
#[test]
fn versioning_case_drops_the_restriction_rows_for_a_pre_feature_declaration() {
    let dep = "\
// @dart=2.19
class Class {}
base class BaseClass {}
final class FinalClass {}
";
    let main = "\
// @dart=2.19
import 'package:t/db.dart';

class ImplementsBase implements BaseClass {}
class ImplementsFinal implements FinalClass {}
";
    let reports = analyze(&[
        RigLib {
            uri: "package:t/main.dart",
            source: main,
        },
        RigLib {
            uri: "package:t/db.dart",
            source: dep,
        },
    ]);
    assert_eq!(
        modifier_rows(&reports[0]),
        0,
        "the entry reports no restriction row for a pre-feature declaring \
         library: {}",
        rows(&reports[0]).join("\n")
    );
    assert_eq!(
        modifier_rows(&reports[1]),
        0,
        "the declaring unit reports no restriction row either: {}",
        rows(&reports[1]).join("\n")
    );
}

/// The over-mask guard for the subtype family: a two-generation pre-feature
/// hierarchy consumed by a post-feature subtype — the declaring library's
/// flags are masked, so neither the direct subtype row nor the
/// explicit-ancestor row may fire.
#[test]
fn pre_feature_ancestor_masks_the_subtype_family() {
    let dep = "\
// @dart=2.19
abstract class Class {}
base class BaseClass extends Class {}
";
    let main = "\
import 'package:t/db.dart';

class Sub extends BaseClass {}
";
    let reports = analyze(&[
        RigLib {
            uri: "package:t/main.dart",
            source: main,
        },
        RigLib {
            uri: "package:t/db.dart",
            source: dep,
        },
    ]);
    assert_eq!(
        modifier_rows(&reports[0]),
        0,
        "neither the subtype row nor the explicit-ancestor row may fire for a \
         pre-feature ancestor: {}",
        rows(&reports[0]).join("\n")
    );
}

/// The no-over-mask control: a modern (`// @dart=3.0`) `base` class still
/// couples its subtypes, so a non-`base` out-of-library subtype reports.
#[test]
fn modern_base_class_still_reports_the_subtype_row() {
    let dep = "\
// @dart=3.0
base class BaseClass {}
";
    let main = "\
import 'package:t/db.dart';

class Sub extends BaseClass {}
";
    let reports = analyze(&[
        RigLib {
            uri: "package:t/main.dart",
            source: main,
        },
        RigLib {
            uri: "package:t/db.dart",
            source: dep,
        },
    ]);
    let subtype = reports[0]
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "subtype_of_base_or_final_is_not_base_final_or_sealed")
        .count();
    assert_eq!(
        subtype,
        1,
        "a 3.0 `base` class still restricts an out-of-library subtype: {}",
        rows(&reports[0]).join("\n")
    );
}
