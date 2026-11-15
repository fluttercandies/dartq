//! #4304 pins: the promotion gate splits its unattributable-head answer.
//!
//! #4294 made every head the flow layer cannot attribute step optimistically:
//! a lossy (prefixed-import) spelling and a bare name two reachable files
//! declare got the same name-level `Some(to)` answer. The #4295
//! differential proved that answer masks a real member-error class — a
//! prefixed tested head whose unrelated class still has the member (`p14`),
//! and a bare collision the unit's own scope cannot pin (`p1`/`p2`/`p3`,
//! where the oracle resolves the reference to `InvalidType` and reports the
//! member against the declared type).
//!
//! The split, pinned here:
//!
//! * a PREFIXED spelling keeps the strict refusal in the legacy fold
//!   (`w is u.Image` reports `fit` on `Widget`, `p14`/`p16`), except FROM a
//!   top standing type, where every tested spelling sits under the declared
//!   type and the step is valid (`Object value; value is u.LocalizedText`
//!   promotes — the corpus's `Object`-declared receivers);
//! * a bare collision the analyzed library's scope pins to the registered
//!   slot is no collision at all — the strict resolved gate stands (`p7`,
//!   whose hidden `widgets.Image` claim must not relax a decidable pair);
//! * a bare collision whose scope names a single OTHER declaration has no
//!   resolved verdict but a real class in scope — both folds keep the
//!   optimistic step (`p6`, whose `uiOnly` row must stay on `Image`);
//! * a bare collision the scope cannot pin refuses the step in the SOUND
//!   fold and keeps the optimistic step in the LEGACY fold, reproducing the
//!   oracle's 2.12 member-row suppression under the `ambiguous_import`
//!   error and its 3.9 member rows on `Widget` (`p1`/`p2`/`p3`).
//!
//! Oracle provenance: `dart analyze` 3.13.7 over the probe sources this
//! suite's fixtures mirror, at both `sdk: >=2.12 <3.0` and `sdk: >=3.9`
//! package language versions (the 2.12 run reports the `ambiguous_import`
//! error without member rows on the collision, the 3.9 run reports the
//! member rows against `Widget`). The 2.12 rows are position-identical to
//! the fixtures below modulo the leading language marker (the probe packages
//! carried their version in `pubspec.yaml` and no marker).
//!
//! The rig mounts the analyzed unit and its dependencies directly on the
//! production graph shape (`LibraryGraphBuilder` + `Namespaces` +
//! `LibraryTypes::build_with_clauses` + `fold_unit_with_ops`), with the
//! knobs the collision shapes need: import prefixes and `show` clauses, and
//! names-only SDK face units (`face_unit` + `declare_face`, the dart:ui
//! projection the Flutter face-only pin mounts).

#![allow(clippy::semicolon_if_nothing_returned)]
use super::{FileReport, StrictModes, fold_unit_with_ops, merge_source_ordered};
use crate::collector::collect;
use crate::library::ElementIndex;
use crate::model::{FileElements, MicroKind};
use crate::resolved_ops::{ClauseTrees, LibraryTypes, ResolvedTypeOps};
use dartq_language::{Feature, FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_resolution::{
    ClassModifiers, CombinatorClause, Combinators, ExportEdge, FaceKind, ImportEdge,
    LibraryClassModifiers, LibraryGraphBuilder, LibraryId, Namespaces,
};
use dartq_scanner::ScanConfig;
use dartq_source::{FileId, FileTable, StringTable};
use rustc_hash::FxHashMap;
use std::path::Path;

// —— the fixture libraries ——————————————————————————————————————————————

/// The unrelated collided declaration (`ui.Image`): the class a PREFIXED
/// tested head can name, and the slot winner when it is imported first.
const UI: &str = "class Image {\n\
                  \x20 int get width => 0;\n\
                  \x20 int get height => 0;\n\
                  \x20 int get uiOnly => 7;\n\
                  }\n\
                  \n\
                  class Gadget {}\n";

/// The visible declaration set (`widgets.dart`): `Widget` and the
/// subtype-of-`Widget` `Image` the collisions compete with.
const WIDGETS: &str = "class Widget {}\n\
                       \n\
                       class Image extends Widget {\n\
                       \x20 double? get width => 1;\n\
                       \x20 double? get height => 2;\n\
                       \x20 String get fit => 'cover';\n\
                       \x20 String get repeat => 'noRepeat';\n\
                       }\n";

const SOLO: &str = "class Solo {\n  int known = 1;\n}\n";

/// The Flutter-shaped real library: the same `Widget`/`Image` pair the
/// face-only pin collides with `dart:ui`'s names-only `Image`.
const FLUTTER: &str = "class Widget {}\n\
                       \n\
                       class Image extends Widget {\n\
                       \x20 double? get width => 1;\n\
                       \x20 String get fit => 'cover';\n\
                       }\n";

// —— the minimal graph rig ———————————————————————————————————————————————

/// One mounted dependency of the fixture's main unit.
struct Dep<'a> {
    uri: &'a str,
    /// The main import's `as` prefix.
    prefix: Option<&'a str>,
    /// The main import's `show` clause; empty imports unfiltered.
    import_show: &'a [&'a str],
    /// `Some(source)` is an indexed library; `None` a names-only SDK face
    /// declaring `face`.
    source: Option<&'a str>,
    face: &'a [&'a str],
    /// `(target uri, show names)` export directives; an empty show list
    /// exports the target unfiltered.
    exports: &'a [(&'a str, &'a [&'a str])],
}

impl<'a> Dep<'a> {
    fn plain(uri: &'a str, source: &'a str) -> Self {
        Self {
            uri,
            prefix: None,
            import_show: &[],
            source: Some(source),
            face: &[],
            exports: &[],
        }
    }

    fn prefixed(uri: &'a str, prefix: &'a str, source: &'a str) -> Self {
        Self {
            prefix: Some(prefix),
            ..Self::plain(uri, source)
        }
    }

    fn showing(uri: &'a str, names: &'a [&'a str], source: &'a str) -> Self {
        Self {
            import_show: names,
            ..Self::plain(uri, source)
        }
    }

    fn face(uri: &'a str, prefix: &'a str, names: &'a [&'a str]) -> Self {
        Self {
            uri,
            prefix: Some(prefix),
            import_show: &[],
            source: None,
            face: names,
            exports: &[],
        }
    }

    fn exporting(uri: &'a str, exports: &'a [(&'a str, &'a [&'a str])]) -> Self {
        Self {
            exports,
            ..Self::plain(uri, "")
        }
    }
}

/// Diagnoses `source` with `deps` mounted as sibling libraries of
/// `package:t/main.dart`, the fixture's own `// @dart=X.Y` marker deciding
/// the language version (and with it the flow fold and the sound gate).
/// `mounts` are libraries reachable through a dependency's `export` rather
/// than a main import (the re-export face's declaring file).
fn report(source: &str, deps: &[Dep<'_>], mounts: &[(&str, &str)]) -> FileReport {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
    let scanned = scan(source, file);
    let version = language_version(source);
    let set = FeatureSet::for_language_version(version);
    let features = ParserFeatures::from_feature_set(set);
    let elements = collect(source, &scanned.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib = builder.intern("package:t/main.dart");
    let mut libs: FxHashMap<&str, LibraryId> = FxHashMap::default();
    let mut dep_files: FxHashMap<&str, FileId> = FxHashMap::default();
    for dep in deps {
        libs.insert(dep.uri, builder.intern(dep.uri));
        dep_files.insert(dep.uri, files.intern(Path::new(dep.uri)));
    }
    for &(uri, _) in mounts {
        libs.insert(uri, builder.intern(uri));
        dep_files.insert(uri, files.intern(Path::new(uri)));
    }
    let mut index = ElementIndex::default();
    let mut clauses = ClauseTrees::empty();
    index.insert(file, elements.clone());
    builder.unit(lib, file);
    declare(&mut builder, lib, file, &elements);
    crate::capture_clause_trees(
        file,
        source,
        &scanned.tokens,
        features,
        &strings,
        &mut clauses,
    );
    for &(uri, mount_source) in mounts {
        let mount_lib = libs[uri];
        let mount_file = dep_files[uri];
        let mount_scanned = scan(mount_source, mount_file);
        let mount_elements = collect(mount_source, &mount_scanned.tokens, features, &strings);
        index.insert(mount_file, mount_elements.clone());
        builder.unit(mount_lib, mount_file);
        declare(&mut builder, mount_lib, mount_file, &mount_elements);
        crate::capture_clause_trees(
            mount_file,
            mount_source,
            &mount_scanned.tokens,
            features,
            &strings,
            &mut clauses,
        );
    }
    for dep in deps {
        let dep_lib = libs[dep.uri];
        let dep_file = dep_files[dep.uri];
        if let Some(dep_source) = dep.source {
            let dep_scanned = scan(dep_source, dep_file);
            let dep_elements = collect(dep_source, &dep_scanned.tokens, features, &strings);
            index.insert(dep_file, dep_elements.clone());
            builder.unit(dep_lib, dep_file);
            declare(&mut builder, dep_lib, dep_file, &dep_elements);
            crate::capture_clause_trees(
                dep_file,
                dep_source,
                &dep_scanned.tokens,
                features,
                &strings,
                &mut clauses,
            );
        } else {
            // The production SDK posture: the synthetic face file rides
            // the element index with an empty model, so the namespace
            // layer discriminates on `is_face_file`, never an index miss.
            index.insert(dep_file, FileElements::default());
            builder.face_unit(dep_lib, dep_file);
            for &name in dep.face {
                builder.declare_face(dep_lib, strings.intern(name), dep_file, FaceKind::Class);
            }
        }
    }
    for dep in deps {
        for &(target, show) in dep.exports {
            builder.export(
                libs[dep.uri],
                ExportEdge {
                    target: Some(libs[target]),
                    combinators: combinator(show, &strings),
                },
            );
        }
    }
    for dep in deps {
        builder.import(
            lib,
            ImportEdge {
                target: Some(libs[dep.uri]),
                prefix: dep.prefix.map(|prefix| strings.intern(prefix)),
                deferred: false,
                non_library: false,
                combinators: combinator(dep.import_show, &strings),
            },
        );
    }
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    let types =
        LibraryTypes::build_with_clauses(&graph, &namespaces, &index, &strings, lib, &clauses);
    let ops = ResolvedTypeOps::new(&types, &strings, file);
    let fold = fold_unit_with_ops(
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
        set.is_enabled(Feature::InferenceUpdate2),
        Some(&ops),
    );
    let mut report = fold.report;
    report.diagnostics = merge_source_ordered(fold.parse_diagnostics, report.diagnostics);
    report
}

/// Declares one unit's top-level names into the graph — the production
/// registration shape (containers as classes, everything else plain).
fn declare(
    builder: &mut LibraryGraphBuilder<'_>,
    lib: LibraryId,
    file: FileId,
    elements: &FileElements,
) {
    for (name, decl) in elements.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        if matches!(
            decl.kind,
            MicroKind::Class | MicroKind::Enum | MicroKind::Mixin
        ) {
            builder.declare_class(lib, name, file, ClassModifiers::default());
        } else {
            builder.declare(lib, name, file);
        }
    }
}

/// An unfiltered combinator for an empty `show` list, a `show` set
/// otherwise.
fn combinator(show: &[&str], strings: &StringTable) -> Combinators {
    if show.is_empty() {
        return Combinators::unfiltered();
    }
    let names: Box<[dartq_source::StringId]> =
        show.iter().map(|name| strings.intern(name)).collect();
    Combinators::new([CombinatorClause::Show(names)], strings)
}

/// The fixture's leading `// @dart=X.Y` marker, else the latest version
/// (the corpus convention the shared rig uses).
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

fn scan(source: &str, file: FileId) -> dartq_scanner::ScanResult {
    let scanned = dartq_scanner::scan(
        source,
        ScanConfig {
            enable_triple_shift: FeatureSet::for_language_version(language_version(source))
                .is_enabled(Feature::TripleShift),
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

// —— row helpers ——————————————————————————————————————————————————————————

/// One code's rows in source order: `(message, line, column)`.
fn rows(report: &FileReport, source: &str, code: &str) -> Vec<(String, u32, u32)> {
    let mut found: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code.name == code)
        .collect();
    found.sort_by_key(|diagnostic| diagnostic.range.offset);
    found
        .into_iter()
        .map(|diagnostic| {
            let (line, column) = line_col(source, diagnostic.range.offset);
            (diagnostic.message.to_string(), line, column)
        })
        .collect()
}

fn line_col(source: &str, offset: u32) -> (u32, u32) {
    let (mut line, mut column, mut seen) = (1, 1, 0);
    for ch in source.chars() {
        if seen >= offset {
            break;
        }
        if ch == '\n' {
            line += 1;
            column = 1;
        } else {
            column += ch.len_utf16() as u32;
        }
        seen += ch.len_utf16() as u32;
    }
    (line, column)
}

fn getters(report: &FileReport, source: &str) -> Vec<(String, u32, u32)> {
    rows(report, source, "undefined_getter")
}

// —— D1: the PREFIXED head keeps the strict refusal ——————————————————————

/// `p14`/`p16`: the declared receiver is BARE `Widget`; only the tested
/// head is prefixed (`u.Image`, an unrelated class). The oracle refuses the
/// promotion and reports the member against `Widget` — `fit` exists only on
/// the collision partner `widgets.Image`, which is exactly the member the
/// optimistic step used to mask.
const P14_P16_LEGACY: &str = r"// @dart=2.12
String p14(Widget w) {
  if (w is u.Image) {
    return w.fit;
  }
  return '';
}

String p16(Widget w) {
  if (w is u.Image) {
    return w.gizmo;
  }
  return '';
}
";

#[test]
fn prefixed_tested_head_reports_on_the_declared_type() {
    let report = report(
        P14_P16_LEGACY,
        &[
            Dep::prefixed("package:t/ui.dart", "u", UI),
            Dep::plain("package:t/widgets.dart", WIDGETS),
        ],
        &[],
    );
    assert_eq!(
        getters(&report, P14_P16_LEGACY),
        vec![
            (
                "The getter 'fit' isn't defined for the type 'Widget'.".to_owned(),
                4,
                14,
            ),
            (
                "The getter 'gizmo' isn't defined for the type 'Widget'.".to_owned(),
                11,
                14,
            ),
        ]
    );
}

/// The prefixed-only declaration the top-receiver pin promotes to.
const LOCALIZED: &str = "class LocalizedText {\n  String resolve() => 'x';\n}\n";

/// A promotion FROM a top type is valid for every tested spelling, so the
/// lossy refusal must not refuse it: the corpus's `Object`-declared
/// receivers keep their promotion, and `value.resolve` stays silent.
const TOP_RECEIVER_LEGACY: &str = r"// @dart=2.12
String topReceiver(Object value) {
  if (value is u.LocalizedText) {
    return value.resolve();
  }
  return '';
}
";

#[test]
fn lossy_head_over_a_top_standing_type_still_promotes() {
    let report = report(
        TOP_RECEIVER_LEGACY,
        &[Dep::prefixed("package:t/localized.dart", "u", LOCALIZED)],
        &[],
    );
    assert_eq!(
        rows(&report, TOP_RECEIVER_LEGACY, "undefined_method"),
        Vec::new()
    );
    assert_eq!(getters(&report, TOP_RECEIVER_LEGACY), Vec::new());
}

/// `p7`: the visible `Image` is the unrelated `ui.Image` (the hidden
/// `widgets.Image` is reachable only through a `show Widget` import). The
/// collision claim must NOT relax the decidable pair — the strict resolved
/// gate refuses at every language version.
const P7: &str = r"String p7(Widget w) {
  if (w is Image) {
    return w.fit;
  }
  return '';
}
";

fn p7_report(version: &str) -> FileReport {
    let source = format!("// @dart={version}\n{P7}");
    report(
        &source,
        &[
            Dep::plain("package:t/ui.dart", UI),
            Dep::exporting(
                "package:t/widgets_show.dart",
                &[("package:t/widgets.dart", &["Widget"])],
            ),
        ],
        &[("package:t/widgets.dart", WIDGETS)],
    )
}

#[test]
fn hidden_collision_whose_scope_names_the_slot_keeps_the_strict_gate_legacy() {
    let source = format!("// @dart=2.12\n{P7}");
    let report = p7_report("2.12");
    assert_eq!(
        getters(&report, &source),
        vec![(
            "The getter 'fit' isn't defined for the type 'Widget'.".to_owned(),
            4,
            14,
        )]
    );
}

#[test]
fn hidden_collision_whose_scope_names_the_slot_keeps_the_strict_gate_sound() {
    let source = format!("// @dart=3.9\n{P7}");
    let report = p7_report("3.9");
    assert_eq!(
        getters(&report, &source),
        vec![(
            "The getter 'fit' isn't defined for the type 'Widget'.".to_owned(),
            4,
            14,
        )]
    );
}

// —— the FOREIGN collision keeps the optimistic step (p6) —————————————————

/// `p6`: `ui.Image` is reachable but hidden behind `show Gadget`; the
/// visible `Image` is `widgets.Image`, a real subtype of `Widget`. The
/// registered slot is the unrelated class, but a real declaration is in
/// scope — both folds keep the optimistic step, and the member row stays on
/// the promoted `Image`.
const P6: &str = r"String p6(Widget w) {
  if (w is Image) {
    return w.uiOnly;
  }
  return '';
}
";

fn p6_report(version: &str) -> FileReport {
    let source = format!("// @dart={version}\n{P6}");
    report(
        &source,
        &[
            Dep::showing("package:t/ui.dart", &["Gadget"], UI),
            Dep::plain("package:t/widgets.dart", WIDGETS),
        ],
        &[],
    )
}

#[test]
fn foreign_collision_keeps_the_optimistic_step_legacy() {
    let source = format!("// @dart=2.12\n{P6}");
    let report = p6_report("2.12");
    assert_eq!(
        getters(&report, &source),
        vec![(
            "The getter 'uiOnly' isn't defined for the type 'Image'.".to_owned(),
            4,
            14,
        )]
    );
}

#[test]
fn foreign_collision_keeps_the_optimistic_step_sound() {
    let source = format!("// @dart=3.9\n{P6}");
    let report = p6_report("3.9");
    assert_eq!(
        getters(&report, &source),
        vec![(
            "The getter 'uiOnly' isn't defined for the type 'Image'.".to_owned(),
            4,
            14,
        )]
    );
}

// —— D3: the UNATTRIBUTABLE collision ————————————————————————————————————

/// `p1`: two unprefixed imports export `Image`; the oracle reports the
/// `ambiguous_import` error and, at 3.9, the member rows against `Widget`.
/// At 2.12 the member rows stay suppressed (the ambiguous reference is not
/// a promotion target), which the legacy fold reproduces.
const P1: &str = r"String p1WidgetMember(Widget w) {
  if (w is Image) {
    return w.fit;
  }
  return '';
}

int p1UiMember(Widget w) {
  if (w is Image) {
    return w.uiOnly;
  }
  return 0;
}
";

/// `p2`: the member exists on neither collided declaration.
const P2: &str = r"String p2(Widget w) {
  if (w is Image) {
    return w.gizmo;
  }
  return '';
}
";

/// `p3`: `is!` guard + early return, then the member read.
const P3: &str = r"String p3(Widget w) {
  if (w is! Image) {
    return 'none';
  }
  return w.fit;
}
";

fn collided(version: &str, body: &str) -> (String, FileReport) {
    let source = format!("// @dart={version}\n{body}");
    let report = report(
        &source,
        &[
            Dep::plain("package:t/ui.dart", UI),
            Dep::plain("package:t/widgets.dart", WIDGETS),
        ],
        &[],
    );
    (source, report)
}

#[test]
fn ambiguous_collision_suppresses_member_rows_legacy() {
    for body in [P1, P2, P3] {
        let (source, report) = collided("2.12", body);
        assert_eq!(getters(&report, &source), Vec::new(), "{body}");
    }
}

#[test]
fn ambiguous_collision_reports_on_the_declared_type_sound() {
    let (source, report) = collided("3.9", P1);
    assert_eq!(
        getters(&report, &source),
        vec![
            (
                "The getter 'fit' isn't defined for the type 'Widget'.".to_owned(),
                4,
                14,
            ),
            (
                "The getter 'uiOnly' isn't defined for the type 'Widget'.".to_owned(),
                11,
                14,
            ),
        ]
    );
    let (source, report) = collided("3.9", P2);
    assert_eq!(
        getters(&report, &source),
        vec![(
            "The getter 'gizmo' isn't defined for the type 'Widget'.".to_owned(),
            4,
            14,
        )]
    );
    let (source, report) = collided("3.9", P3);
    assert_eq!(
        getters(&report, &source),
        vec![(
            "The getter 'fit' isn't defined for the type 'Widget'.".to_owned(),
            6,
            12,
        )]
    );
}

// —— the exact controls ——————————————————————————————————————————————————

/// `p5`: an unambiguous head keeps its row (the fix is collision-scoped).
const P5: &str = r"int p5(Solo s) {
  if (s is Solo) {
    return s.missing;
  }
  return s.known;
}
";

#[test]
fn unambiguous_wrong_member_still_reports() {
    let source = format!("// @dart=2.12\n{P5}");
    let report = report(&source, &[Dep::plain("package:t/solo.dart", SOLO)], &[]);
    assert_eq!(
        getters(&report, &source),
        vec![(
            "The getter 'missing' isn't defined for the type 'Solo'.".to_owned(),
            4,
            14,
        )]
    );
}

/// `p10`: a single-declaration `Image` keeps the resolved promotion — the
/// member row rides the promoted class, `Image`.
const P10: &str = r"String p10(Widget w) {
  if (w is Image) {
    return w.uiOnly;
  }
  return '';
}
";

#[test]
fn single_declaration_name_keeps_the_resolved_promotion() {
    let source = format!("// @dart=2.12\n{P10}");
    let report = report(
        &source,
        &[Dep::plain("package:t/widgets.dart", WIDGETS)],
        &[],
    );
    assert_eq!(
        getters(&report, &source),
        vec![(
            "The getter 'uiOnly' isn't defined for the type 'Image'.".to_owned(),
            4,
            14,
        )]
    );
}

// —— the Flutter face-only shape ————————————————————————————————————————

/// The `dart:ui` face + `package:flutter` real class collision: the bare
/// `Image` is the real widget class (the face only contributes the
/// prefixed spelling), so the guard's continuation must keep its promotion
/// and the two oracle rows must report — `toByteData` (13:7 in the oracle
/// probe; 11:7 here, the fixture drops the two import lines and adds the
/// language marker) and `gizmo` (19:14 → 17:14).
const FLUTTER_FACE: &str = r"String fWidgetMember(Widget w) {
  if (w is! Image) {
    return '';
  }
  return '${w.width}';
}

void fUiOnlyMember(Widget w) {
  if (w is Image) {
    w.toByteData();
  }
}

String fNeitherMember(Widget w) {
  if (w is Image) {
    return w.gizmo;
  }
  return '';
}
";

#[test]
fn flutter_face_collision_keeps_the_member_rows() {
    let source = format!("// @dart=2.12\n{FLUTTER_FACE}");
    let report = report(
        &source,
        &[
            Dep::face("dart:ui", "ui", &["Image"]),
            Dep::plain("package:flutter/widgets.dart", FLUTTER),
        ],
        &[],
    );
    let methods = rows(&report, &source, "undefined_method");
    assert_eq!(methods.len(), 1, "{methods:?}");
    assert_eq!(methods[0].1, 11);
    assert_eq!(methods[0].2, 7);
    let getters = getters(&report, &source);
    let gizmo = getters
        .iter()
        .find(|(message, _, _)| message.contains("'gizmo'"))
        .expect("the gizmo row reports");
    assert_eq!((gizmo.1, gizmo.2), (17, 14));
}
