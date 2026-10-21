//! The #2442 pins: the member-resolution false-positive family of the
//! analyzer-corpus census ⑨ — member faces whose receiver static type is
//! the oracle's `InvalidType` (an unresolvable `on` name / extended type)
//! answered silently upstream, while dartq reported
//! `undefined_getter`/`undefined_method` on the spelling's nearest
//! container.
//!
//! Three sub-clusters fixed in `check_instance_member`:
//!
//! - (C) extension-`this` member reads through an on-name the diagnosing
//!   scope cannot resolve (`extension E on CommentToken` where
//!   `CommentToken` arrives only through a broken export): the tail's
//!   frozen consult treated every non-`Defined` verdict as a reportable
//!   miss naming the on type — `Unknown` (the `InvalidType` posture, the
//!   corpus's `lib/src/util/yaml.dart` `this.value` rows) emitted. The
//!   gate now falls through only on a decidable `Absent`.
//! - (D) `Stream.transform` missing from the frozen `dart:async` face
//!   table: a member read `this.transform(...)` inside
//!   `extension StreamExtension<T> on Stream<T>` resolved `Absent` on a
//!   member the real SDK `Stream` carries — added to `SDK_FACES_EXTRA`.
//! - (E) the operator-merged row (`this.offset + wordOffset` folding the
//!   `offset` getter read into `Intent::Operator("+")`) bypassed the
//!   on-face union entirely: inside a mixin/extension body it named the
//!   container (`'M'`/`'E'`) instead of walking the `on`/`extended` face —
//!   the corpus's `ignore_info.dart` eight rows. The arm now runs the
//!   same union: a carried member suppresses, an undecidable on-face
//!   silences, and only a decidable `Absent` reports — naming the `on`
//!   type like the plain tail.
//!
//! Oracle provenance: /tmp/probe2442 — `uc`/`vb`/`vc`/`ud` (oracle silent
//! on `this.offset` inside `mixin M on C`/`extension E on C`, dartq
//! emitted `'M'`/`'E'`), `xb` (`extension … on CommentToken` with the
//! on-name behind a broken export: oracle silent), `ua`/`ub`/`va`/`ta`
//! (operator-merged arm parity on the on-face walk).

use crate::collector::collect;
use crate::diagnose::{FileReport, StrictModes, diagnose_with};
use crate::library::ElementIndex;
use crate::model::MicroKind;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_resolution::{
    CombinatorClause, Combinators, ImportEdge, LibraryClassModifiers, LibraryGraphBuilder,
    Namespaces,
};
use dartq_scanner::ScanConfig;
use dartq_source::{FileTable, StringTable};
use std::path::Path;

/// The one-library rig: `source` is the sole analyzed file of
/// `package:t`, no imports.
fn unit_report(source: &str) -> FileReport {
    units_report(&[], source)
}

/// `units` are extra `(path, source)` libraries the main unit imports
/// unfiltered (the two-file rig for a resolved on-face declared outside
/// the diagnosing file, and for the broken-edge posture).
fn units_report(units: &[(&str, &str)], source: &str) -> FileReport {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
    let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
    let features = ParserFeatures::from_feature_set(set);
    let scan_cfg = ScanConfig {
        enable_triple_shift: true,
        enable_augmentations: true,
    };
    let scanned = dartq_scanner::scan(source, scan_cfg, file);
    assert!(
        scanned.diagnostics.is_empty(),
        "scanner diagnostics: {source:?}"
    );
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
    for (i, (path, unit_source)) in units.iter().enumerate() {
        let unit_file = files.intern(Path::new(path));
        let scanned_u = dartq_scanner::scan(unit_source, scan_cfg, unit_file);
        assert!(
            scanned_u.diagnostics.is_empty(),
            "scanner diagnostics: {unit_source:?}"
        );
        let elements_u = collect(unit_source, &scanned_u.tokens, features, &strings);
        index.insert(unit_file, elements_u.clone());
        let lib_u = builder.intern(&format!("package:t/unit{i}.dart"));
        builder.unit(lib_u, unit_file);
        for (name, decl) in elements_u.declarations() {
            if decl.kind == MicroKind::TypeParam {
                continue;
            }
            builder.declare(lib_u, name, unit_file);
        }
        builder.import(
            lib,
            ImportEdge {
                target: Some(lib_u),
                prefix: None,
                deferred: false,
                combinators: Combinators::unfiltered(),
            },
        );
    }
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
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

/// [`units_report`] with the main unit's only edge a `show`-filtered
/// import whose target is `None` (the corpus's broken-export posture:
/// the name resolves nowhere, but the spelling itself is consumed by the
/// edge — no `undefined_class` for the on-name, matching `xb`).
fn broken_show_report(shown: &str, source: &str) -> FileReport {
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
    let mut index = ElementIndex::default();
    index.insert(file, elements.clone());
    for (name, decl) in elements.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(lib, name, file);
    }
    builder.import(
        lib,
        ImportEdge {
            target: None,
            prefix: None,
            deferred: false,
            combinators: Combinators::new(
                [CombinatorClause::Show(
                    vec![strings.intern(shown)].into_boxed_slice(),
                )],
                &strings,
            ),
        },
    );
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
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

fn code_names(report: &FileReport) -> Vec<&'static str> {
    report
        .diagnostics
        .iter()
        .map(|d| d.code.name)
        .collect::<Vec<_>>()
}

fn member_miss<'a>(report: &'a FileReport, needle: &str) -> String {
    report
        .diagnostics
        .iter()
        .filter(|d| {
            matches!(
                d.code.name,
                "undefined_getter" | "undefined_method" | "undefined_setter"
            )
        })
        .map(|d| d.message.to_string())
        .find(|m| m.contains(needle))
        .unwrap_or_default()
}

/// (C) `this.m` inside `extension E on Broken` — the on-name resolves
/// only through a broken export edge, so `this`'s static type is the
/// oracle's `InvalidType` and every member face stays silent (the yaml
/// corpus's `this.value` rows).
#[test]
fn extension_this_broken_on_type_stays_silent() {
    let report = broken_show_report(
        "CommentToken",
        "\
import 'token.dart';
extension CommentTokenExtension on CommentToken {
  void f() { this.offset; }
}
",
    );
    assert!(
        member_miss(&report, "offset").is_empty(),
        "{:?}",
        code_names(&report)
    );
    assert!(
        member_miss(&report, "CommentToken").is_empty(),
        "{:?}",
        code_names(&report)
    );
}

/// (C) the same posture at the receiver-declaring site: the broken
/// `on` name keeps even the plain read silent — the miss records but
/// reports nothing (the `UndefinedGuard` never names `InvalidType`'s
/// interface).
#[test]
fn extension_this_broken_on_type_nested_closure_stays_silent() {
    let report = broken_show_report(
        "CommentToken",
        "\
import 'token.dart';
extension CommentTokenExtension on CommentToken {
  void f() {
    void inner() { this.offset; }
    inner();
  }
}
",
    );
    assert!(
        member_miss(&report, "offset").is_empty(),
        "{:?}",
        code_names(&report)
    );
}

/// Control for (C): a decidable miss against a *resolved* on type still
/// reports — naming the on type, never the extension (the oracle's
/// `InstanceLookupDomain` reads `this`'s static type).
#[test]
fn extension_this_decidable_miss_names_on_type() {
    let report = unit_report(
        "\
class C {}
extension E on C {
  void f() { this.nope; }
}
",
    );
    assert!(
        member_miss(&report, "'nope' isn't defined for the type 'C'").len() > 0,
        "{:?}",
        report
            .diagnostics
            .iter()
            .map(|d| &d.message)
            .collect::<Vec<_>>()
    );
}

/// Control for (C): a carried member on the resolved on-face resolves
/// `this.m` — no row at all.
#[test]
fn extension_this_carried_member_resolves() {
    let report = unit_report(
        "\
class C { int get offset => 0; }
extension E on C {
  void f() { this.offset; }
}
",
    );
    assert!(
        member_miss(&report, "offset").is_empty(),
        "{:?}",
        code_names(&report)
    );
}

/// (D) `Stream.transform` rides the frozen `dart:async` face: a member
/// read through an SDK `Stream`-typed receiver — and through an
/// extension body's `this` — resolves (the corpus's
/// `stream.dart whereType` row). The bare unit's `Stream` never
/// resolves (no dart:async import in the rig), so the walk chain-breaks
/// at the anchor and the frozen table owns the answer.
#[test]
fn stream_transform_member_resolves() {
    let report = unit_report(
        "\
extension E<T> on Stream<T> {
  void f() { this.transform; }
}
",
    );
    assert!(
        member_miss(&report, "transform").is_empty(),
        "{:?}",
        code_names(&report)
    );
}

/// (E) the operator-merged row runs the on-face union: `this.offset + 1`
/// inside `mixin M on C` resolves the getter through `C`'s face — the
/// corpus's `ignore_info.dart` eight rows' shape (there the on-name
/// `CommentToken` is the broken-import posture of the two tests above).
#[test]
fn mixin_this_operator_fold_resolves_through_on_face() {
    let report = unit_report(
        "\
class C { int get offset => 0; int get wordOffset => 1; }
mixin M on C {
  int get g {
    var offset = 5;
    void inner() { print(this.offset + wordOffset); }
    inner();
    return offset;
  }
}
",
    );
    assert!(
        member_miss(&report, "offset").is_empty(),
        "{:?}",
        code_names(&report)
    );
    assert!(
        member_miss(&report, "wordOffset").is_empty(),
        "{:?}",
        code_names(&report)
    );
}

/// (E) the same fold inside an extension body: `this.m + 1` resolves
/// through the extended type.
#[test]
fn extension_this_operator_fold_resolves_through_on_face() {
    let report = unit_report(
        "\
class C { int get offset => 0; }
extension E on C {
  int get g {
    void inner() { print(this.offset + 1); }
    inner();
    return 0;
  }
}
",
    );
    assert!(
        member_miss(&report, "offset").is_empty(),
        "{:?}",
        code_names(&report)
    );
}

/// Control for (E): a decidable operator-merged miss still reports —
/// naming the `on` type like the plain tail (the miss's honest
/// interface), never the extension name.
#[test]
fn extension_this_operator_decidable_miss_names_on_type() {
    let report = unit_report(
        "\
class C {}
extension E on C {
  int get g {
    void inner() { print(this.nope + 1); }
    inner();
    return 0;
  }
}
",
    );
    assert!(
        member_miss(&report, "'nope' isn't defined for the type 'C'").len() > 0,
        "{:?}",
        report
            .diagnostics
            .iter()
            .map(|d| &d.message)
            .collect::<Vec<_>>()
    );
}

/// Control for (E): the `Object` operator face keeps its exemption —
/// `==` and `hashCode` fold into `Intent::Operator` rows on every
/// receiver without a walk. (The function's return type stays unwritten:
/// the rig seeds no dart:core, and the pin's face is the operator fold,
/// not the annotation.)
#[test]
fn object_operator_fold_stays_silent() {
    let report = unit_report(
        "\
class C {}
extension E on C {
  f() => this == this && this.hashCode > 0;
}
",
    );
    assert!(
        report.diagnostics.is_empty(),
        "{:?}",
        code_names(&report)
    );
}
