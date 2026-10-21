//! The #2484 pins: the listener-family residual batch (#2478/#2480).
//!
//! Oracle provenance: probe2484 (`dart analyze` over the tiny rigs,
//! Dart 3.13/3.47 fvm oracle), anchored against upstream Dart SDK source,
//! pkg/analyzer/lib/src/dart/constant/constant_verifier.dart and
//! pkg/analyzer/lib/src/dart/constant/potentially_constant.dart, plus
//! pkg/analyzer/lib/src/dart/resolver/yield_statement_resolver.dart:
//!
//! - `_validateConstructorInitializers` runs under
//!   `node.constKeyword != null` only (probe2484 p1d/p1e: a non-const
//!   constructor's `super(f)` is silent), walks each super/redirect
//!   argument through the potentially-constant collector
//!   (`_reportNotPotentialConstantsArguments` → `invalid_constant` at the
//!   failing leaf — p1c: a non-const top-level name in a const
//!   constructor's `super(g)` reports `invalid_constant`), and
//!   `_nameExpression` counts the enclosing const constructor's formal
//!   parameters as potentially constant (p1a/p1f: `super(f)` and
//!   `super(f + 1)` over a const-ctor formal are silent; p1b: the
//!   redirect twin is silent too).
//! - `_checkForYieldOfInvalidType` reads `expression.typeOrThrow` after
//!   the shared analyzer's yield-star rewrite — the source's flow-aware
//!   static type (p2a/p2c: a null-guard-promoted `yield* src` is
//!   silent; p2b/p2d: the failing face names the source's own type,
//!   `Iterable<int>?` / `String`).
//! - a record literal inside a const-constructor initializer is never
//!   `isConst`, so the `_ConstLiteralVerifier` element faces
//!   (`non_constant_record_field`) never run there; the initializer's
//!   potentially-constant scan owns the leaf with `invalid_constant`
//!   (p4a), while a true const declaration context keeps
//!   `non_constant_record_field` (p4b — pinned green by
//!   `ticket_2478::non_const_record_field_in_const_declaration_still_fires`,
//!   the other direction of this selection).

use super::*;
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
/// `package:t`, no imports (the #2453 rig — core types are declared
/// locally so their nullability faces resolve).
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

fn codes(report: &FileReport, name: &str) -> Vec<(u32, u32)> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == name)
        .map(|d| (d.range.offset, d.range.length))
        .collect()
}

// —— the initializer arglists' potentially-constant scan —————————————————————

/// A const constructor's formal parameter passed through the
/// `super(...)` initializer argument list is potentially constant
/// (probe2484 p1a: oracle-silent), so the scan must not fault it — the
/// formals bind as opaque names in the scan's lookup (the same overlay
/// `check_const_elements` rides). A binary over the formal rides the
/// same pass (probe2484 p1f: `super(f + 1)` silent).
#[test]
fn const_ctor_formal_super_arglist_stays_silent() {
    let source = "\
class int {}

class A {
  final int? x;
  const A(this.x);
}

class B extends A {
  const B({required int f}) : super(f);
}
";
    let report = unit_report(source);
    assert!(report.diagnostics.is_empty(), "{:?}", report.diagnostics);
}

/// The redirect twin (`: this.named(f)`): the oracle's
/// `RedirectingConstructorInvocation` arm scans the same way and the
/// formal passes (probe2484 p1b: oracle-silent); the engine's face stays
/// silent on the shape.
#[test]
fn const_ctor_formal_redirect_arglist_stays_silent() {
    let source = "\
class int {}

class C {
  final int? v;
  const C.named(this.v);
  const C({required int f}) : this.named(f);
}
";
    let report = unit_report(source);
    assert!(report.diagnostics.is_empty(), "{:?}", report.diagnostics);
}

/// The honesty face: a genuinely non-constant name in a const
/// constructor's super argument list collects `invalid_constant` at the
/// failing leaf (probe2484 p1c: `error - invalid_constant` at the `g`
/// argument, the sole row).
#[test]
fn ctor_super_arglist_nonconst_name_still_fires() {
    let source = "\
class int {}

int? g;

class A {
  final int? x;
  const A(this.x);
}

class B extends A {
  const B() : super(g);
}
";
    let report = unit_report(source);
    let at = source.find("g)").unwrap() as u32;
    assert_eq!(
        codes(&report, "invalid_constant"),
        vec![(at, 1)],
        "{:?}",
        report.diagnostics
    );
    assert_eq!(report.diagnostics.len(), 1, "{:?}", report.diagnostics);
}

/// The scan is the const constructor's own gate (upstream
/// `_validateConstructorInitializers` runs under `constKeyword != null`
/// only; probe2484 p1d: the non-const constructor's `super(f)` over the
/// initializing formal is silent): a non-const constructor's initializer
/// argument list never runs the potentially-constant scan.
#[test]
fn nonconst_ctor_super_arglist_stays_silent() {
    let source = "\
class int {}

class A {
  A(int x);
}

class B extends A {
  int f;
  B(this.f) : super(f);
}
";
    let report = unit_report(source);
    assert!(report.diagnostics.is_empty(), "{:?}", report.diagnostics);
}

// —— the yield* staged verdict ————————————————————————————————————————————————

/// The oracle's `yield*` verdict reads the source's flow-aware static
/// type (probe2484 p2a: the null-guard-promoted `yield* src` is
/// silent), so the star arm stages beside the plain yield and the
/// promoted read cleans the verdict at the teardown join.
#[test]
fn yield_each_guarded_source_stays_silent() {
    let source = "\
class int {}
class Iterable<T> {}

Iterable<int> gen(Iterable<int>? src) sync* {
  if (src != null) {
    yield* src;
  }
}
";
    let report = unit_report(source);
    assert!(report.diagnostics.is_empty(), "{:?}", report.diagnostics);
}

/// Without a promotion the parse-time wrap decides as before
/// (probe2484 p2b: the oracle reports at the source — the nullable
/// source fails the implied-sequence assignability) — the row stays
/// anchored at the source expression.
#[test]
fn yield_each_unguarded_nullable_still_fires() {
    let source = "\
class int {}
class Iterable<T> {}

Iterable<int> gen(Iterable<int>? src) sync* {
  yield* src;
}
";
    let report = unit_report(source);
    let at = source.rfind("src").unwrap() as u32;
    assert_eq!(
        codes(&report, "yield_of_invalid_type"),
        vec![(at, 3)],
        "{:?}",
        report.diagnostics
    );
}

/// A promotion that still misses keeps the row with the promoted type as
/// the implied sequence — the official implied type IS the source's
/// resolved static type (probe2484 p2b/p2d message shape: the failing
/// face names the source's own type, never a wrap of it).
#[test]
fn yield_each_promoted_miss_keeps_row_with_promoted_type() {
    let source = "\
class int {}
class String {}
class Iterable<T> {}

Iterable<int> gen(Iterable<String>? src) sync* {
  if (src != null) {
    yield* src;
  }
}
";
    let report = unit_report(source);
    let at = source.rfind("src").unwrap() as u32;
    assert_eq!(
        codes(&report, "yield_of_invalid_type"),
        vec![(at, 3)],
        "{:?}",
        report.diagnostics
    );
    let d = report
        .diagnostics
        .iter()
        .find(|d| d.code.name == "yield_of_invalid_type")
        .unwrap();
    assert_eq!(
        d.message,
        "The type 'Iterable<String>' implied by the 'yield*' expression must be assignable to 'Iterable<int>'."
    );
}

// —— the initializer record field's code selection ———————————————————————————

/// A record field inside a const constructor's initializer is gated by
/// the initializer's potentially-constant scan, whose face is
/// `invalid_constant` at the failing leaf (probe2484 p4a) — the record
/// literal there is never `isConst`, so the element face
/// (`non_constant_record_field`) never substitutes. The other direction
/// — a true const declaration context keeping
/// `non_constant_record_field` — is pinned by
/// `ticket_2478::non_const_record_field_in_const_declaration_still_fires`.
#[test]
fn record_init_nonconst_field_reports_invalid_constant() {
    let source = "\
class int {}

int? g;

class C {
  final ({int? a}) rec;
  const C() : rec = (a: g);
}
";
    let report = unit_report(source);
    let at = source.find("g)").unwrap() as u32;
    assert_eq!(
        codes(&report, "invalid_constant"),
        vec![(at, 1)],
        "{:?}",
        report.diagnostics
    );
}
