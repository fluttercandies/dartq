//! #3324 pins — `isStrictlyNonNullable`'s missing `FutureOr`/extension-type
//! arms on the model level.
//!
//! The frozen verdict is upstream `TypeSystem.isStrictlyNonNullable`
//! (upstream Dart SDK source,
//! pkg/analyzer/lib/src/dart/element/type_system.dart:1255-1274): beyond
//! the plain `?`/sentinel faces [`Ty::is_strictly_non_nullable`] already
//! answers, a non-nullable `FutureOr<T>` recurses into `T`
//! (`FutureOr<int?>` is NOT strictly non-nullable — the
//! `invalid_null_aware_operator` spread face over it is the EXTRA
//! direction), a bare `FutureOr` reads `FutureOr<dynamic>` (nullable), and
//! an extension type is strictly non-nullable exactly when it declares
//! interfaces (`interfaces.isNotEmpty`).
//!
//! [`FileElements::is_strictly_non_nullable`] is the model-level seam.
//! The engine's bound-aware delegate (`strictly_non_nullable`,
//! `diagnose/engine_core.rs`) still routes plain faces through the plain
//! [`Ty::is_strictly_non_nullable`] — wiring it through this helper is the
//! #3316-lane follow-up — so:
//! - the helper-level pins below prove the new arms directly;
//! - the end-to-end pins cover only the faces that answer identically
//!   before and after that wiring (the non-regression set);
//! - the headline repro pin (`[...?FutureOr<int?>]` stays silent) rides
//!   `#[ignore]` and flips green the moment the delegate routes through
//!   the helper.

use crate::collector::collect;
use crate::diagnose::test_rig::{CORE_PRELUDE, report_ops};
use crate::diagnose::types::FileReport;
use crate::model::{ArgListId, FileElements, Ty};
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_scanner::{ScanConfig, scan};
use dartq_source::{FileTable, StringTable};
use std::path::Path;

// —— helper-level rig ——————————————————————————————————————————————————————

/// Collects `source` into the model the helper decides over: the string
/// table is the id universe, the elements' own pool is the `Ty`
/// argument-list universe (the same pairing the engine's per-unit
/// `FileElements::type_args` keeps).
fn model(source: &str) -> (StringTable, FileElements) {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
    let scanned = scan(
        source,
        ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: true,
        },
        file,
    );
    let features = ParserFeatures::from_feature_set(FeatureSet::for_language_version(
        LanguageVersion::new(3, 15),
    ));
    let elements = collect(source, &scanned.tokens, features, &strings);
    (strings, elements)
}

/// A `?`-free named type without arguments.
fn named(text: &str, strings: &StringTable) -> Ty {
    Ty::Named {
        name: strings.intern(text),
        nullable: false,
        args: ArgListId::EMPTY,
    }
}

/// A `?`-suffixed named type without arguments.
fn named_q(text: &str, strings: &StringTable) -> Ty {
    Ty::Named {
        name: strings.intern(text),
        nullable: true,
        args: ArgListId::EMPTY,
    }
}

/// The dart:async `FutureOr<T>` spelling over `arg`, interned into the
/// elements' own pool.
fn future_or_of(arg: Ty, strings: &StringTable, elements: &mut FileElements) -> Ty {
    Ty::Named {
        name: strings.intern("FutureOr"),
        nullable: false,
        args: elements.type_args_mut().intern(&[arg]),
    }
}

// —— helper level: the FutureOr arm —————————————————————————————————————————

/// `FutureOr<int?>` is NOT strictly non-nullable — the union inherits its
/// argument's nullable face (the `invalid_null_aware_operator` EXTRA
/// direction this arm removes).
#[test]
fn future_or_of_nullable_argument_is_not_strictly_non_nullable() {
    let source = "class I {}\nvoid f() {}\n";
    let (strings, mut elements) = model(source);
    let ty = future_or_of(named_q("int", &strings), &strings, &mut elements);
    assert!(
        !elements.is_strictly_non_nullable(&strings, ty),
        "`FutureOr<int?>` must read nullable-possible"
    );
}

/// `FutureOr<int>` stays strictly non-nullable — the non-nullable argument
/// keeps the union's face (the regression twin).
#[test]
fn future_or_of_non_nullable_argument_stays_strictly_non_nullable() {
    let source = "class I {}\nvoid f() {}\n";
    let (strings, mut elements) = model(source);
    let ty = future_or_of(named("int", &strings), &strings, &mut elements);
    assert!(
        elements.is_strictly_non_nullable(&strings, ty),
        "`FutureOr<int>` must stay strictly non-nullable"
    );
}

/// A bare `FutureOr` reads `FutureOr<dynamic>` — nullable (the
/// `overrides` face's written-nullability convention).
#[test]
fn bare_future_or_reads_the_dynamic_argument() {
    let source = "class I {}\nvoid f() {}\n";
    let (strings, elements) = model(source);
    assert!(
        !elements.is_strictly_non_nullable(&strings, named("FutureOr", &strings)),
        "a bare `FutureOr` must read `FutureOr<dynamic>`"
    );
}

/// `FutureOr<FutureOr<int?>>` recurses through both union layers; the
/// fully non-nullable chain stays strictly non-nullable.
#[test]
fn nested_future_or_chains_recurse_their_base() {
    let source = "class I {}\nvoid f() {}\n";
    let (strings, mut elements) = model(source);
    let nullable_chain = future_or_of(
        future_or_of(named_q("int", &strings), &strings, &mut elements),
        &strings,
        &mut elements,
    );
    assert!(
        !elements.is_strictly_non_nullable(&strings, nullable_chain),
        "`FutureOr<FutureOr<int?>>` must read nullable-possible"
    );
    let strict_chain = future_or_of(
        future_or_of(named("int", &strings), &strings, &mut elements),
        &strings,
        &mut elements,
    );
    assert!(
        elements.is_strictly_non_nullable(&strings, strict_chain),
        "`FutureOr<FutureOr<int>>` must stay strictly non-nullable"
    );
}

/// A declared container named `FutureOr` shadows the dart:async union: the
/// in-file class wins resolution and takes the generic arm (a non-nullable
/// user class is strictly non-nullable regardless of its arguments).
#[test]
fn shadowed_future_or_takes_the_generic_arm() {
    let source = "class FutureOr<T> {}\nvoid f() {}\n";
    let (strings, mut elements) = model(source);
    let ty = future_or_of(named_q("int", &strings), &strings, &mut elements);
    assert!(
        elements.is_strictly_non_nullable(&strings, ty),
        "a user `class FutureOr` is a plain interface face, not the union"
    );
}

// —— helper level: the extension-type arm ——————————————————————————————————

/// An extension type declaring interfaces is strictly non-nullable
/// (`interfaces.isNotEmpty`); the `?` projection still kills the face, a
/// plain class keeps the generic arm, and an interfaces-less extension
/// type reads nullable-possible.
#[test]
fn extension_type_decides_through_its_interfaces() {
    let source = "\
class I {}
class C {}
extension type ET(int x) implements I {}
extension type EBare(int x) {}
void f() {}
";
    let (strings, elements) = model(source);
    assert!(
        elements.is_strictly_non_nullable(&strings, named("ET", &strings)),
        "`ET implements I` is strictly non-nullable"
    );
    assert!(
        !elements.is_strictly_non_nullable(&strings, named_q("ET", &strings)),
        "the `?` projection stays nullable"
    );
    assert!(
        elements.is_strictly_non_nullable(&strings, named("C", &strings)),
        "a plain class keeps the generic arm"
    );
    assert!(
        !elements.is_strictly_non_nullable(&strings, named("EBare", &strings)),
        "an interfaces-less extension type reads nullable-possible"
    );
}

// —— helper level: the unchanged plain faces —————————————————————————————————

/// Every face the plain [`Ty::is_strictly_non_nullable`] already answers
/// stays identical through the helper: sentinels and `?` shapes false,
/// `Never` and `?`-free compound shapes true.
#[test]
fn plain_faces_answer_unchanged_through_the_helper() {
    let source = "class I {}\nvoid f() {}\n";
    let (strings, elements) = model(source);
    assert!(elements.is_strictly_non_nullable(&strings, named("int", &strings)));
    assert!(!elements.is_strictly_non_nullable(&strings, named_q("int", &strings)));
    assert!(!elements.is_strictly_non_nullable(&strings, Ty::Unknown));
    assert!(!elements.is_strictly_non_nullable(&strings, Ty::Null));
    assert!(!elements.is_strictly_non_nullable(&strings, Ty::Dynamic));
    assert!(!elements.is_strictly_non_nullable(&strings, Ty::Void));
    assert!(elements.is_strictly_non_nullable(&strings, Ty::Never));
}

// —— engine level: the non-regression set ——————————————————————————————————

/// The absolute offset of the `nth` occurrence (0-based) of `needle` in the
/// fixture as the rig analyzes it: `report_ops` prefixes [`CORE_PRELUDE`],
/// so every fixture-local offset shifts by the prelude's length.
fn at(fixture: &str, needle: &str, nth: usize) -> u32 {
    let mut from = 0;
    for _ in 0..nth {
        from += fixture[from..]
            .find(needle)
            .expect("needle appears in the fixture")
            + needle.len();
    }
    let hit = from + fixture[from..].find(needle).expect("needle appears in the fixture");
    u32::try_from(CORE_PRELUDE.len() + hit).expect("fixture offsets fit u32")
}

/// The rows of one code name, as `(offset, length)`, ascending.
fn anchors(report: &FileReport, code: &str) -> Vec<(u32, u32)> {
    let mut at: Vec<(u32, u32)> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| (d.range.offset, d.range.end() - d.range.offset))
        .collect();
    at.sort_unstable();
    at
}

/// `FutureOr<int>` keeps the dead spread report — the union over a
/// non-nullable argument stays strictly non-nullable through the engine's
/// delegate, before and after the #3324 helper routing.
#[test]
fn future_or_non_nullable_receiver_reports_the_dead_spread() {
    let fixture = "\
void f(FutureOr<int> x) {
  [...?x];
}
";
    let report = report_ops(fixture);
    assert_eq!(
        anchors(&report, "invalid_null_aware_operator"),
        vec![(at(fixture, "...?", 0), 4)],
        "`[...?x]` over `FutureOr<int>` reports at the `...?` token; got: {:?}",
        report.diagnostics
    );
}

/// A plain nullable receiver keeps the spread's `?` meaningful — the
/// unchanged baseline face.
#[test]
fn plain_nullable_receiver_keeps_the_spread_silent() {
    let fixture = "\
void f(int? x) {
  [...?x];
}
";
    let report = report_ops(fixture);
    assert!(
        anchors(&report, "invalid_null_aware_operator").is_empty(),
        "`[...?x]` over `int?` stays silent; got: {:?}",
        report.diagnostics
    );
}

/// An extension-type receiver WITH interfaces reports the dead spread
/// today (the plain shape face answers true) and must keep reporting once
/// the delegate routes through the helper (`interfaces.isNotEmpty` →
/// true) — the ExtensionType arm's non-regression witness.
#[test]
fn extension_type_with_interfaces_receiver_reports_the_dead_spread() {
    let fixture = "\
class I {}
extension type ET(int x) implements I {}
void f(ET e) {
  [...?e];
}
";
    let report = report_ops(fixture);
    assert_eq!(
        anchors(&report, "invalid_null_aware_operator"),
        vec![(at(fixture, "...?", 0), 4)],
        "`[...?e]` over an interfaces-bearing extension type reports; got: {:?}",
        report.diagnostics
    );
}

/// The `!` face routes the same delegate: a `FutureOr<int>` operand's
/// assertion is dead (`unnecessary_non_null_assertion`), the nullable twin
/// stays silent — one verdict function across both null-aware faces.
#[test]
fn null_assertion_over_future_or_faces_match_the_spread_face() {
    let dead = "\
void f(FutureOr<int> y) {
  y!;
}
";
    let report = report_ops(dead);
    assert_eq!(
        anchors(&report, "unnecessary_non_null_assertion").len(),
        1,
        "`y!` over `FutureOr<int>` reports the dead assertion; got: {:?}",
        report.diagnostics
    );
    let live = "\
void f(int? y) {
  y!;
}
";
    let report = report_ops(live);
    assert!(
        anchors(&report, "unnecessary_non_null_assertion").is_empty()
            && anchors(&report, "null_check_always_fails").is_empty(),
        "`y!` over `int?` stays silent; got: {:?}",
        report.diagnostics
    );
}

// —— engine level: the delegate-routing acceptance (ignored follow-up) ——————

/// The headline repro: `[...?x]` over `FutureOr<int?>` must stay SILENT
/// (the union inherits the argument's nullable face). The engine's
/// delegate still answers the plain shape face here, so the row is EXTRA
/// until the #3316-lane follow-up routes `strictly_non_nullable` through
/// [`FileElements::is_strictly_non_nullable`] — remove the `#[ignore]`
/// with that wiring.
#[test]
#[ignore = "#3324 follow-up: the engine_core delegate routing (see FileElements::is_strictly_non_nullable)"]
fn future_or_nullable_argument_spread_stays_silent() {
    let fixture = "\
void f(FutureOr<int?> x) {
  [...?x];
}
";
    let report = report_ops(fixture);
    assert!(
        anchors(&report, "invalid_null_aware_operator").is_empty(),
        "`[...?x]` over `FutureOr<int?>` must stay silent; got: {:?}",
        report.diagnostics
    );
}
