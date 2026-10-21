//! The #2473 pins: the bare member scope inside `mixin`/`extension`
//! bodies resolves through `this`'s full interface, not only the
//! enclosing container's own member map (census G2, the w-drift
//! `json()`/`toSql`/`fromSql`/`listen`/`byName` rows on Dart 3.13.1).
//!
//! `resolve_bare` already computed the union — `walk_members` over the
//! container's own climb, then `on_face_walk` over the `on`/constraint
//! faces — but the invocation face asked `walk_members` alone, so a bare
//! send whose member lives on the `on` type (`dartCast<String>()` inside
//! `extension E on Expression<int>`) never recorded a receiver: its
//! generic inference parameters, the explicit-argument solutions, and the
//! member's declared return all dropped, leaving the invocation's result
//! typed as the raw head (`Expression`, never `Expression<String>`) and
//! every applied-face member downstream reporting `undefined_method`.
//! [`Engine::bare_member_walk`] is the shared union the invocation face
//! now reads, and `on_face_walk`'s mixin arm seeds the `implements` faces
//! beside the `on` constraints (the oracle's `MixinMemberScope` resolves
//! a bare name through both).

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

/// G2a: `dartCast<String>().json()` — the bare send inside an extension
/// body resolves `dartCast` through the `on` type's face (the union arm),
/// the explicit `<String>` instantiates its `Expression<D2>` return to
/// `Expression<String>`, and the `on Expression<String>` extension's
/// `json` applies silently (the drift `json1.dart` rows).
#[test]
fn on_face_method_send_instantiates_its_generic_return() {
    let source = "\
class Expression<D> {
  Expression<D2> dartCast<D2>() => Expression<D2>();
}

extension JsonExt on Expression<String> {
  Expression<String> json() => Expression<String>();
}

extension JsonbExt on Expression<int> {
  Expression<String> m() {
    return dartCast<String>().json();
  }
}
";
    let names = code_names(&report(source));
    assert!(!names.contains(&"undefined_method"), "{names:?}");
}

/// The keep-reporting guard for G2a: a bare send whose member the `on`
/// type genuinely lacks keeps its `undefined_method` face — the union
/// consults the `on` face but a clean miss still misses.
#[test]
fn on_face_miss_still_reports_undefined_method() {
    let source = "\
class Expression<D> {
  void real() {}
}

extension JsonbExt on Expression<int> {
  void m() {
    missing();
  }
}
";
    let names = code_names(&report(source));
    assert!(names.contains(&"undefined_method"), "{names:?}");
}

/// G2b: `mixin M implements Base { ... baseMember(); }` — the mixin's
/// bare member scope carries the `implements` face's interface (the
/// oracle's own verdict on the probe: silent), exactly like `on`.
#[test]
fn mixin_implements_face_joins_the_bare_member_scope() {
    let source = "\
abstract class Base {
  void baseMember();
}

mixin Impl implements Base {
  void other() {
    baseMember();
  }
}
";
    let names = code_names(&report(source));
    assert!(!names.contains(&"undefined_method"), "{names:?}");
    assert!(!names.contains(&"undefined_identifier"), "{names:?}");
}

/// The transitive G2b shape: `mixin Outer implements Inner` where
/// `Inner on Base` — `Outer`'s scope reaches `Base`'s members through
/// `Inner`'s `on` constraint (the oracle's silent verdict).
#[test]
fn mixin_implements_climbs_the_inner_on_constraint() {
    let source = "\
abstract class Base {
  void baseMember();
}

mixin Inner on Base {
  void deep() {
    baseMember();
  }
}

mixin Outer implements Inner {
  void other() {
    baseMember();
  }
}
";
    let names = code_names(&report(source));
    assert!(!names.contains(&"undefined_method"), "{names:?}");
    assert!(!names.contains(&"undefined_identifier"), "{names:?}");
}

/// The keep-reporting guard for G2b: a name the `implements` face
/// genuinely lacks still reports `undefined_method` against the mixin's
/// constraint interface.
#[test]
fn mixin_implements_miss_still_reports() {
    let source = "\
abstract class Base {
  void baseMember();
}

mixin Impl implements Base {
  void other() {
    missing();
  }
}
";
    let names = code_names(&report(source));
    assert!(names.contains(&"undefined_method"), "{names:?}");
}
