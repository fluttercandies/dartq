//! The #2566 pins: UNDEFINED_* member-resolution sweep — fill in as roots close. The rig
//! declares no imports, so every pin spells custom types only.

use super::*;
use crate::collector::collect;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_resolution::LibraryGraphBuilder;
use dartq_scanner::ScanConfig;
use dartq_source::FileTable;
use std::path::Path;

/// Diagnoses `source` as the sole file of a library with no imports —
/// the single-file rig (`ticket_2564_tests`' shape).
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

fn count_code(report: &FileReport, name: &str) -> usize {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == name)
        .count()
}

fn messages(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .map(|d| format!("{}: {}", d.code.name, d.message))
        .collect()
}

/// The #2566 root: a mixin CLASS applied through `with` contributes its
/// own `with` faces to the interface — `class C extends Object with A`
/// where `mixin class A = Object with M` exposes M's members exactly like
/// a directly-written `with M` (the named-application rule; the oracle
/// resolves the chain, tests/language/mixin5_test.dart's shape). Before
/// the fix the mixin hop's miss-path queued only `on`/`implements` faces,
/// so the walk answered Missing and the verdict over-reported
/// `undefined_method`.
#[test]
fn mixin_class_application_exposes_underlying_mixin_members() {
    let source = "\
mixin M<T> {
  T m() { return throw 0; }
}

mixin class A<U, V> = Object with M<Map<U, V>>;

class C<T> extends Object with A<T, Set<T>> {}

void f() {
  var c = C<int>();
  c.m();
}
";
    let result = report(source);
    assert_eq!(
        count_code(&result, "undefined_method"),
        0,
        "{:?}",
        messages(&result)
    );
}

/// The same rule one level deeper: a named application whose `with` face
/// is itself a named application (`class C<T> = S with B<...>` over
/// `mixin class B = Object with A<...>` over `mixin class A = Object
/// with M<...>` — mixin5_test's exact shape). The face-list climb expands
/// each applied mixin's own `with` faces transitively.
#[test]
fn nested_mixin_application_chain_resolves_deep_members() {
    let source = "\
class I<T> {}
class J<T> {}
class K<T> {}
class S<T> {}

mixin M<T> {
  m() { return T; }
}

mixin class A<U, V> = Object with M<Map<U, V>> implements I<V>;

mixin class B<T> = Object with A<T, Set<T>> implements J<T>;

class C<T> = S<List<T>> with B<List<T>> implements K<T>;

void f() {
  var c = C<int>();
  c.m();
}
";
    let result = report(source);
    assert_eq!(
        count_code(&result, "undefined_method"),
        0,
        "{:?}",
        messages(&result)
    );
}

/// A member that no face of the expanded chain declares still reports —
/// the added `with` faces widen the FOUND face, never the miss verdict.
#[test]
fn mixin_class_application_chain_still_reports_genuine_miss() {
    let source = "\
mixin M<T> {
  T m() { return throw 0; }
}

mixin class A<U, V> = Object with M<Map<U, V>>;

class C<T> extends Object with A<T, Set<T>> {}

void f() {
  var c = C<int>();
  c.absent();
}
";
    let result = report(source);
    assert_eq!(
        count_code(&result, "undefined_method"),
        1,
        "{:?}",
        messages(&result)
    );
}

/// The `@override` face's climb (`face_member`) shares the rule: an
/// `@override` member that only an applied mixin class's underlying mixin
/// declares is overriding (the oracle's `getOverridden` walks the
/// application's interface), so `override_on_non_overriding_method` must
/// not fire.
#[test]
fn override_face_climbs_applied_mixin_class_underlying_mixin() {
    let source = "\
mixin M<T> {
  void m() {}
}

mixin class A<U, V> = Object with M<Map<U, V>>;

class C<T> extends Object with A<T, Set<T>> {
  @override
  void m() {}
}
";
    let result = report(source);
    assert_eq!(
        count_code(&result, "override_on_non_overriding_method"),
        0,
        "{:?}",
        messages(&result)
    );
}

/// The `extends` face of the same rule: a class whose SUPERCLASS is a
/// named application (`class F extends B<String>` over `class B<V> =
/// Object with A<V>` over `mixin class A<U> = Object with M<U>` —
/// tests/language/mixin2_test.dart). The climb reaches B's `with` face A
/// through the regular mixin hop, and A's own `with` face M now joins the
/// pending levels.
#[test]
fn superclass_named_application_exposes_underlying_mixin() {
    let source = "\
mixin M<T> {
  T t() { return throw 0; }
}

mixin class A<U> = Object with M<U>;

class B<V> = Object with A<V>;

class F extends B<String> {}

void f() {
  F x = F();
  x.t();
}
";
    let result = report(source);
    assert_eq!(
        count_code(&result, "undefined_method"),
        0,
        "{:?}",
        messages(&result)
    );
}
