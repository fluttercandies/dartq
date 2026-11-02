//! #3227 pins — F9-D: a same-library `base`/`final` superdeclaration loses
//! its whole modifier plan (the `basic_restrictions_A04_t04` shape: local
//! `abstract base class` emits zero rows where the imported-twin `t03`
//! emits the full expected set).
//!
//! Upstream drives `BaseOrFinalTypeVerifier.checkElement` and the
//! `_checkFor*OutsideOfLibrary` block on every container with resolved
//! clause faces — there is NO file-level suppression arm: a declaration in
//! the same library reports the same `subtype_of_base_or_final_is_not_
//! base_final_or_sealed` / `invalid_use_of_type_outside_library` family rows
//! it would report across a library boundary (upstream Dart SDK source,
//! `pkg/analyzer/lib/src/generated/error_verifier.dart:2327-2370` +
//! `pkg/analyzer/lib/src/error/base_or_final_type_verifier.dart:40-74`).
//!
//! The rig mirrors `ticket_3219_tests.rs`: each fixture library is scanned
//! and collected at its own `// @dart` version and the posture map mirrors
//! the engine's `build_class_modifiers`.
//!
//! Registered through `diagnose/tests.rs`'s `#[path]` block: the brief's
//! named files are live lanes out of this ticket's write scope, and
//! `#[path]` keeps the module inside the `diagnose` subtree so
//! `crate::diagnose::*` resolves.

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
        let class_modifiers =
            FeatureSet::for_language_version(version).is_enabled(Feature::ClassModifiers);
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
fn modifier_rows(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| MODIFIER_CODES.contains(&d.code.name))
        .map(|d| d.code.name.to_string())
        .collect()
}

fn subtype_rows(report: &FileReport) -> usize {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "subtype_of_base_or_final_is_not_base_final_or_sealed")
        .count()
}

/// The `t04` discriminator shape, minimized: a same-library
/// `abstract base class` extended by a plain subtype owes one
/// `subtype_of_base_or_final_is_not_base_final_or_sealed` row. Pre-fix the
/// whole plan emitted zero rows for every container in the file.
#[test]
fn same_library_abstract_base_class_reports_the_subtype_row() {
    let main = "\
abstract base class AbstractBaseClass {}
class ExtendsBaseClass extends AbstractBaseClass {}
";
    let reports = analyze(&[RigLib {
        uri: "package:t/main.dart",
        source: main,
    }]);
    assert_eq!(
        subtype_rows(&reports[0]),
        1,
        "a local `abstract base class` still restricts its same-library \
         subtype: {}",
        rows(&reports[0]).join("\n")
    );
}

/// The positive control: the same declarations imported across a library
/// boundary still fire — the `t03` shape must not regress.
#[test]
fn imported_abstract_base_class_still_reports_the_subtype_row() {
    let dep = "\
abstract base class AbstractBaseClass {}
";
    let main = "\
import 'package:t/db.dart';

class ExtendsBaseClass extends AbstractBaseClass {}
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
        subtype_rows(&reports[0]),
        1,
        "an imported `abstract base class` still restricts the subtype: {}",
        rows(&reports[0]).join("\n")
    );
}

/// The `with` + `on` clause faces in the same file: `base mixin` consumed
/// through `with` and `on` reports their subtype rows too.
#[test]
fn same_library_base_mixin_with_and_on_faces_report() {
    let main = "\
base mixin BaseMixin {}
class ClassWithBaseMixin with BaseMixin {}
mixin MixinOnBase on BaseMixin {}
";
    let reports = analyze(&[RigLib {
        uri: "package:t/main.dart",
        source: main,
    }]);
    let found = modifier_rows(&reports[0]);
    assert_eq!(
        found.len(),
        2,
        "`with BaseMixin` and `on BaseMixin` each owe a subtype row: {}",
        rows(&reports[0]).join("\n")
    );
}

/// The `final`-face row: a local `final class` reports
/// `subtype_of_base_or_final_is_not_base_final_or_sealed` for a non-carrier
/// subtype in the same library (the t05/t07 face shape, same-library side).
#[test]
fn same_library_final_class_reports_the_subtype_row() {
    let main = "\
final class FinalClass {}
class ExtendsFinal extends FinalClass {}
";
    let reports = analyze(&[RigLib {
        uri: "package:t/main.dart",
        source: main,
    }]);
    assert_eq!(
        subtype_rows(&reports[0]),
        1,
        "a local `final class` still restricts its same-library subtype: {}",
        rows(&reports[0]).join("\n")
    );
}

/// The verbatim `basic_restrictions_A04_t04` declaration set — the whole
/// same-library `abstract base`/`abstract base mixin class` plan the corpus
/// loses: every `extends`/`with`/`implements`/`on` face owes its row.
/// `subtype_of_base_or_final_is_not_base_final_or_sealed` fires once per
/// non-carrier subtype under a base ancestor (the 22 subtype cases), and
/// the mixin-drive rows spell `mixin_subtype_of_base_is_not_base`.
#[test]
fn a04_t04_verbatim_same_library_plan_fires() {
    let main = "\
abstract base class AbstractBaseClass {}
abstract base mixin class AbstractBaseMixinClass {}

class ExtendsBaseClass extends AbstractBaseClass {}
interface class InterfaceExtendsBaseClass extends AbstractBaseClass {}
abstract class AbstractExtendsBaseClass extends AbstractBaseClass {}
abstract interface class AbstractInterfaceExtendsBaseClass extends AbstractBaseClass {}
class ExtendsAbstractBaseMixinClass extends AbstractBaseMixinClass {}
interface class InterfaceExtendsAbstractBaseMixinClass extends AbstractBaseMixinClass {}
abstract class AbstractExtendsAbstractBaseMixinClass extends AbstractBaseMixinClass {}
abstract interface class AbstractInterfaceExtendsAbstractBaseMixinClass extends AbstractBaseMixinClass {}

class ImplementsBaseClass implements AbstractBaseClass {}
interface class InterfaceImplementsBaseClass implements AbstractBaseClass {}
abstract class AbstractImplementsBaseClass implements AbstractBaseClass {}
abstract interface class AbstractInterfaceImplementsBaseClass implements AbstractBaseClass {}

class ClassWithBaseMixinClass with AbstractBaseMixinClass {}
interface class InterfaceClassWithBaseMixinClass with AbstractBaseMixinClass {}
abstract class AbstractClassWithBaseMixinClass with AbstractBaseMixinClass {}
abstract interface class AbstractInterfaceClassWithBaseMixinClass with AbstractBaseMixinClass {}

mixin MixinOnBaseClass on AbstractBaseClass {}
mixin MixinOnBaseMixinClass on AbstractBaseMixinClass {}
mixin MixinImplementsBaseClass implements AbstractBaseClass {}
mixin MixinImplementsBaseMixinClass implements AbstractBaseMixinClass {}

mixin class MixinClassImplementsBaseClass implements AbstractBaseClass {}
mixin class MixinClassImplementsBaseMixinClass implements AbstractBaseMixinClass {}
abstract mixin class AbstractMixinClassImplementsBaseClass implements AbstractBaseClass {}
abstract mixin class AbstractMixinClassImplementsBaseMixinClass implements AbstractBaseMixinClass {}
";
    let reports = analyze(&[RigLib {
        uri: "package:t/main.dart",
        source: main,
    }]);
    let found = modifier_rows(&reports[0]);
    // Every listed declaration is a non-carrier subtype under a `base`
    // ancestor — each owes exactly one subtype-family row (the two
    // `abstract base`/`abstract base mixin class` declarations themselves
    // are the supers, not subtypes).
    assert_eq!(
        found.len(),
        24,
        "the whole same-library plan must fire for the t04 set: {}",
        rows(&reports[0]).join("\n")
    );
}

/// The `with` + `implements` combination the #3220 recon flagged as the
/// suspected deferred-slot loss shape (`class C with M implements I`).
#[test]
fn with_and_implements_on_one_header_keeps_its_plan() {
    let main = "\
abstract base class AbstractBaseClass {}
abstract base mixin class AbstractBaseMixinClass {}
class WithImplements with AbstractBaseMixinClass implements AbstractBaseClass {}
";
    let reports = analyze(&[RigLib {
        uri: "package:t/main.dart",
        source: main,
    }]);
    let found = modifier_rows(&reports[0]);
    assert!(
        !found.is_empty(),
        "a header carrying both `with` and `implements` must keep its plan: {}",
        rows(&reports[0]).join("\n")
    );
}
