//! The #3109 pins: INVALID_OVERRIDE family — the named-mixin-application
//! `with`-face chain on the override verdict face, and the
//! `CovariantParametersVerifier` relatedness gate. The rig declares no
//! imports, so every pin spells custom types only.
//!
//! Upstream citations:
//! - the application interface keeps an applied mixin class's own `with`
//!   faces' members (upstream Dart SDK source,
//!   `pkg/analyzer/lib/src/dart/element/inheritance_manager3.dart` — the
//!   named-application interface; tests/language/mixin5_test.dart's shape).
//! - `CovariantParametersVerifier` (upstream Dart SDK source,
//!   `pkg/analyzer/lib/src/error/correct_override.dart:96-138`; wired from
//!   `pkg/analyzer/lib/src/error/inheritance_override.dart:541-544` —
//!   declared-member path only): a `covariant` parameter is exempt from the
//!   function-subtype check but its type must still relate to the super
//!   parameter (`thisType <: superType || superType <: thisType`), else the
//!   oracle reports plain `invalid_override` even for a setter face.

use super::*;
use crate::collector::collect;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_resolution::LibraryGraphBuilder;
use dartq_scanner::ScanConfig;
use dartq_source::FileTable;
use std::path::Path;

/// Diagnoses `source` as the sole file of a library with no imports —
/// the single-file rig (`ticket_2566_tests`' shape).
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

/// A mixin CLASS applied through `with` contributes its own `with` faces
/// to the override face (`mixin class A = Object with M` — `with A`
/// exposes M's members exactly like a written `with M`). The oracle flags
/// `C.m(int)` against `M.m(num)`; before the chain the override climb
/// answered no face member and the verdict suppressed.
#[test]
fn override_face_climbs_applied_mixin_class_with_faces() {
    let source = "\
mixin M {
  void m(num x) {}
}

mixin class A = Object with M;

class C extends Object with A {
  void m(int x) {}
}
";
    let result = report(source);
    assert_eq!(
        count_code(&result, "invalid_override"),
        1,
        "{:?}",
        messages(&result)
    );
}

/// The generic named-application chain (`class D extends C<String>` over
/// `class C<T> = Object with A<List<T>, int>` over `mixin class A<U, V> =
/// Object with M`): the override face climbs D → C → A → M through the
/// pending `with`-face queue.
#[test]
fn override_face_climbs_generic_named_application() {
    let source = "\
mixin M {
  void m(num x) {}
}

mixin class A<U, V> = Object with M;

class C<T> = Object with A<List<T>, int>;

class D extends C<String> {
  void m(int x) {}
}
";
    let result = report(source);
    assert_eq!(
        count_code(&result, "invalid_override"),
        1,
        "{:?}",
        messages(&result)
    );
}

/// A valid override through the same chain stays silent — the widened
/// face widens the FOUND candidate set, never the verdict direction.
#[test]
fn applied_mixin_class_chain_keeps_valid_override_silent() {
    let source = "\
mixin M {
  void m(num x) {}
}

mixin class A = Object with M;

class C extends Object with A {
  void m(num x) {}
}
";
    let result = report(source);
    assert_eq!(
        count_code(&result, "invalid_override"),
        0,
        "{:?}",
        messages(&result)
    );
}

/// `CovariantParametersVerifier` (upstream `correct_override.dart:96-138`):
/// a `covariant` declared parameter whose type relates to the super
/// parameter in neither direction still reports `invalid_override` — the
/// subtype check's `Object?` rewrite does not rescue an unrelated type.
#[test]
fn covariant_parameter_unrelated_type_reports() {
    let source = "\
class S {
  void f(int x) {}
}

class C extends S {
  void f(covariant String x) {}
}
";
    let result = report(source);
    assert_eq!(
        count_code(&result, "invalid_override"),
        1,
        "{:?}",
        messages(&result)
    );
}

/// The related covariant parameter stays silent (`String` narrowing `num`
/// — `num <: String` fails but `String <: num` holds, so the pair relates
/// and the oracle reports nothing).
#[test]
fn covariant_parameter_related_type_silent() {
    let source = "\
class S {
  void f(num x) {}
}

class C extends S {
  void f(covariant int x) {}
}
";
    let result = report(source);
    assert_eq!(
        count_code(&result, "invalid_override"),
        0,
        "{:?}",
        messages(&result)
    );
}

/// The propagated covariance fires the same gate (upstream
/// `parameter.isCovariant` covers the inferred flag — an overriding
/// parameter is covariant when any overridden parameter is): `String`
/// over `S.f(covariant num)` relates in neither direction.
#[test]
fn propagated_covariant_parameter_unrelated_reports() {
    let source = "\
class S {
  void f(covariant num x) {}
}

class D extends S {
  void f(String x) {}
}
";
    let result = report(source);
    assert_eq!(
        count_code(&result, "invalid_override"),
        1,
        "{:?}",
        messages(&result)
    );
}

/// The covariant verifier always reports the plain `invalid_override`
/// code — a covariant SETTER face carries no `_setter` arm upstream
/// (`correct_override.dart:124-133` emits `diag.invalidOverride`).
#[test]
fn covariant_setter_reports_plain_override_code() {
    let source = "\
class S {
  set f(num v) {}
}

class C extends S {
  set f(covariant String v) {}
}
";
    let result = report(source);
    assert_eq!(
        count_code(&result, "invalid_override"),
        1,
        "{:?}",
        messages(&result)
    );
    assert_eq!(
        count_code(&result, "invalid_override_setter"),
        0,
        "{:?}",
        messages(&result)
    );
}

/// A covariant named parameter follows the same relatedness gate
/// (`{covariant String x}` over `{num x}` — unrelated, oracle reports
/// `invalid_override` with the named-parameter function type).
#[test]
fn covariant_named_parameter_unrelated_reports() {
    let source = "\
class S {
  void f({covariant num x = 0}) {}
}

class C extends S {
  void f({covariant String x = \"\"}) {}
}
";
    let result = report(source);
    assert_eq!(
        count_code(&result, "invalid_override"),
        1,
        "{:?}",
        messages(&result)
    );
}

/// A covariant parameter that legitimately narrows stays silent under
/// both faces — the gate only fires when NEITHER subtype direction holds.
#[test]
fn covariant_subtype_narrowing_silent() {
    let source = "\
class S {
  void f(num x) {}
}

class C extends S {
  void f(covariant int x) {}
}
";
    let result = report(source);
    assert_eq!(
        count_code(&result, "invalid_override"),
        0,
        "{:?}",
        messages(&result)
    );
}

/// The concrete-implementation face through the same chain
/// (`invalid_implementation_override`): `class C extends S with A` where
/// `mixin class A = Object with M` puts M's members on the application's
/// interface, so M.m's signature conflict with S.m reports the concrete
/// row at the class name — the `face_entries` enumeration's own
/// `with`-face expansion.
#[test]
fn implementation_override_reads_applied_mixin_class_members() {
    let source = "\
class S {
  void m(num x) {}
}

mixin M {
  void m(int x) {}
}

mixin class A = Object with M;

class C extends S with A {}
";
    let result = report(source);
    assert_eq!(
        count_code(&result, "invalid_implementation_override"),
        1,
        "{:?}",
        messages(&result)
    );
}

/// The transitive application chain (`class D extends C` over `class C =
/// Object with B<int>` over `mixin class B<U> = Object with A<U>` over
/// `mixin class A<T> = Object with M`): each hop's own `with` faces queue
/// transitively, so the override face reaches M through three
/// applications.
#[test]
fn override_face_climbs_transitive_applications() {
    let source = "\
mixin M {
  void m(num x) {}
}

mixin class A<T> = Object with M;

mixin class B<U> = Object with A<U>;

class C = Object with B<int>;

class D extends C {
  void m(int x) {}
}
";
    let result = report(source);
    assert_eq!(
        count_code(&result, "invalid_override"),
        1,
        "{:?}",
        messages(&result)
    );
}
