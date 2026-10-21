//! The #2477 pins: the oracle's second-order derivations over undefined
//! types (census families G6 + G8) — the reachability and condition-fold
//! faces the #2450 dead-code port left adjacent.
//!
//! - G6(i) undefined-scrutinee switch fall-through: a switch whose
//!   scrutinee's written type resolves nowhere is never exhaustive
//!   (`TypeSystem.isAlwaysExhaustive` answers `false` on the oracle's
//!   `InvalidType`), so both returning case bodies still leave the body's
//!   end reachable and `body_might_complete_normally` reports at the
//!   declaring function name (probe `/tmp/probe2477/rig/
//!   p7_undef_enum_switch_might.dart`, dart analyze 3.13.1 — the census's
//!   webview `webview_android_widget.dart:445` row). A resolvable enum
//!   scrutinee keeps the switch closing.
//! - G6(ii) the undefined type-argument face: a written argument that
//!   resolves nowhere is the oracle's `InvalidType` in that position
//!   (`non_type_as_type_argument`), so `FutureOr<UndefinedNav>` is the
//!   vacuous `Invalid`-headed union and `rv is Future<UndefinedNav>` folds
//!   constant-true through the 0c arm (subtype.dart:274-276) — the else
//!   branch reports `dead_code` (probe `p3_is_future_undef_arg.dart`). A
//!   well-typed operand (`int x is Future<UndefinedNav>`) answers the real
//!   relation — false — and keeps both branches live (probe
//!   `p12_undef_arg_matrix.dart` case `a`), and a `dynamic` operand stays
//!   live the same way.
//! - G8 the `identical(v, null)` fold: dart:core `identical` routes
//!   through `equalityOperation_end` like a binary `==` (upstream Dart SDK
//!   source, pkg/analyzer/lib/src/dart/resolver/invocation_inferrer.dart
//!   :556-577), so the disjoint-by-nullability verdict folds the call
//!   constant-false on a non-nullable operand and the true branch reports
//!   `dead_code` — either argument order (probe `p8_identical_null.dart`;
//!   the census's html `dom.dart:446` row). A nullable or `dynamic`
//!   operand keeps both branches live, and the false branch of a nullable
//!   operand's check still promotes (no row on `v.length` in the else).
//! - The keep-honest is-test: a `dynamic` receiver against an undefined
//!   name never folds (the #2450 dynamic face) — `type_test_with_undefined_
//!   name` reports on the spelling and both branches stay live (probe
//!   `p12_isdead`, the corpus's dominant shape).
//!
//! The harness is the #2450 one: the sole file of a library folded with
//! the resolved `TypeOps` mounted, diagnostics read off the joined
//! report.

use super::*;
use crate::collector::collect;
use crate::resolved_ops::{LibraryTypes, ResolvedTypeOps};
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_resolution::{LibraryClassModifiers, LibraryGraphBuilder};
use dartq_scanner::ScanConfig;
use dartq_source::FileTable;
use std::path::Path;

fn scan(source: &str, file: FileId) -> (dartq_scanner::ScanResult, ParserFeatures, FeatureSet) {
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
    (scanned, features, set)
}

/// Diagnoses `source` as the sole file of a library with the resolved
/// `TypeOps` face mounted — the engine path the switch exhaustiveness
/// verdict, the is-check folds and the `identical` condition fold decide
/// through.
fn report_ops(source: &str) -> FileReport {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
    let (scanned, features, set) = scan(source, file);
    let elements = collect(source, &scanned.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib = builder.intern("package:t/main.dart");
    builder.unit(lib, file);
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
    let types = LibraryTypes::build(&graph, &namespaces, &index, &strings, lib);
    let ops = ResolvedTypeOps::new(&types, &strings);
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
        set.is_enabled(dartq_language::Feature::InferenceUpdate2),
        Some(&ops),
    );
    let mut report = fold.report;
    report.diagnostics = merge_source_ordered(fold.parse_diagnostics, report.diagnostics);
    report
}

fn code_names(report: &FileReport) -> Vec<&'static str> {
    report.diagnostics.iter().map(|d| d.code.name).collect()
}

fn rows(report: &FileReport, code: &str) -> Vec<(u32, u32)> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| (d.range.offset, d.range.length))
        .collect()
}

// —— G6(i): the undefined-scrutinee switch keeps the fall-through ——————

/// The corpus face (webview `webview_android_widget.dart:445`): a switch
/// over a parameter whose written type resolves nowhere is not exhaustive
/// (`isAlwaysExhaustive` answers `false` on the oracle's `InvalidType`),
/// so both returning case bodies still leave the body end reachable and
/// `body_might_complete_normally` reports at the declaring name — the
/// probe anchors it exactly there (p7: `1:14`, the function name).
#[test]
fn undefined_scrutinee_switch_reports_body_might() {
    let source = concat!(
        "class Future<T> {}\n",
        "Future<void> g() async {}\n",
        "Future<void> f(JavascriptMode mode) {\n",
        "  switch (mode) {\n",
        "    case JavascriptMode.disabled:\n",
        "      return g();\n",
        "    case JavascriptMode.unrestricted:\n",
        "      return g();\n",
        "  }\n",
        "}\n",
    );
    let report = report_ops(source);
    let might = rows(&report, "body_might_complete_normally");
    let name_offset = source.find("Future<void> f(").map(|found| {
        // The declaring name's span: `f` sits after the return type.
        found + "Future<void> ".len()
    });
    assert!(
        !might.is_empty(),
        "the undefined-scrutinee switch keeps the body's completion live: {:?}",
        report.diagnostics
    );
    if let Some(expected) = name_offset {
        assert!(
            might.iter().any(|&(offset, _)| offset == expected),
            "body_might anchors at the declaring name at {expected}: {might:?}"
        );
    }
}

/// The resolvable twin: the same shape over a real two-case enum closes —
/// the switch is exhaustive, the body end stays unreachable, no row.
#[test]
fn resolvable_enum_switch_stays_silent() {
    let source = concat!(
        "class Future<T> {}\n",
        "enum Mode { a, b }\n",
        "Future<void> g() async {}\n",
        "Future<void> f(Mode mode) {\n",
        "  switch (mode) {\n",
        "    case Mode.a:\n",
        "      return g();\n",
        "    case Mode.b:\n",
        "      return g();\n",
        "  }\n",
        "}\n",
    );
    let report = report_ops(source);
    assert!(
        rows(&report, "body_might_complete_normally").is_empty(),
        "the exhaustive enum switch closes: {:?}",
        report.diagnostics
    );
}

// —— G6(ii): the undefined type-argument fold ————————————————————————

/// `FutureOr<UndefinedNav> is Future<UndefinedNav>` folds constant-true:
/// the undefined written argument is the oracle's `InvalidType` in that
/// position, so the operand is the Invalid-headed `FutureOr` union and the
/// tested `Future<Invalid>` admits it through the 0c arm — the else branch
/// reports `dead_code` (probe p3: the fold pairs with the two
/// `non_type_as_type_argument` rows on the spellings).
#[test]
fn undefined_type_argument_is_test_deads_the_else() {
    let source = concat!(
        "class Future<T> {}\n",
        "void h(FutureOr<UndefinedNav> rv) {\n",
        "  if (rv is Future<UndefinedNav>) {\n",
        "    print(rv);\n",
        "  } else {\n",
        "    print('no');\n",
        "  }\n",
        "}\n",
    );
    let report = report_ops(source);
    let else_start = source.find("else").unwrap() as u32;
    let dead = rows(&report, "dead_code");
    assert!(
        dead.iter().any(|&(offset, _)| offset >= else_start),
        "the else arm of the Invalid-argument is-test is dead: {:?}",
        report.diagnostics
    );
    assert!(
        code_names(&report)
            .iter()
            .filter(|name| **name == "non_type_as_type_argument")
            .count()
                >= 2,
            "both undefined argument spellings report: {:?}",
            report.diagnostics
    );
}

/// The well-typed operand keeps both branches: `int is Future<Invalid>`
/// answers the real relation (false — no fold either way; probe
/// p12_undef_arg_matrix case `a`), so no branch reports dead.
#[test]
fn well_typed_operand_keeps_the_undefined_argument_test_live() {
    let source = concat!(
        "class Future<T> {}\n",
        "void h(int x) {\n",
        "  if (x is Future<UndefinedNav>) {\n",
        "    print('a');\n",
        "  } else {\n",
        "    print('b');\n",
        "  }\n",
        "}\n",
    );
    let report = report_ops(source);
    assert!(
        rows(&report, "dead_code").is_empty(),
        "the real-relation answer keeps both arms live: {:?}",
        report.diagnostics
    );
}

// —— G8: the identical-null condition fold —————————————————————————————

/// `identical(v, null)` on a non-nullable `v` folds constant-false: the
/// then branch reports `dead_code` at its first statement (probe p8 case
/// `a`; the census's html `dom.dart:446` ternary shares the verdict).
#[test]
fn identical_null_on_non_nullable_deads_the_then() {
    let source = concat!(
        "void f(String v) {\n",
        "  if (identical(v, null)) {\n",
        "    print('dead');\n",
        "  }\n",
        "}\n",
    );
    let report = report_ops(source);
    let then_start = source.find("print('dead')").unwrap() as u32;
    let dead = rows(&report, "dead_code");
    assert!(
        dead.iter().any(|&(offset, _)| offset >= then_start),
        "the then branch of identical(v, null) is dead: {:?}",
        report.diagnostics
    );
}

/// The swapped-argument order folds the same way (probe p8 case `d`):
/// `identical(null, v)` on a non-nullable `v` deads the then branch.
#[test]
fn identical_null_swapped_arguments_deads_the_then() {
    let source = concat!(
        "void f(int v) {\n",
        "  if (identical(null, v)) {\n",
        "    print('dead');\n",
        "  }\n",
        "}\n",
    );
    let report = report_ops(source);
    let then_start = source.find("print('dead')").unwrap() as u32;
    let dead = rows(&report, "dead_code");
    assert!(
        dead.iter().any(|&(offset, _)| offset >= then_start),
        "the then branch of identical(null, v) is dead: {:?}",
        report.diagnostics
    );
}

/// The nullable operand keeps both branches live (probe p8 case `b`): the
/// types are not disjoint, so the check is a real null test — and its
/// false branch promotes, keeping `v.length` in the else silent.
#[test]
fn identical_null_on_nullable_keeps_both_branches() {
    let source = concat!(
        "void f(String? v) {\n",
        "  if (identical(v, null)) {\n",
        "    print('a');\n",
        "  } else {\n",
        "    v.length;\n",
        "  }\n",
        "}\n",
    );
    let report = report_ops(source);
    let dead = rows(&report, "dead_code");
    assert!(
        dead.is_empty(),
        "the nullable operand's null check stays live: {:?}",
        report.diagnostics
    );
    assert!(
        code_names(&report)
            .iter()
            .all(|name| !name.ends_with("nullable_value")),
        "the promoted else read reports no null-dereference face: {:?}",
        report.diagnostics
    );
}

/// The `dynamic` operand keeps both branches live (probe p8 case `c`):
/// `dynamic` is not disjoint from `Null`, so no verdict folds.
#[test]
fn identical_null_on_dynamic_keeps_both_branches() {
    let source = concat!(
        "void f(dynamic v) {\n",
        "  if (identical(v, null)) {\n",
        "    print('a');\n",
        "  } else {\n",
        "    print('b');\n",
        "  }\n",
        "}\n",
    );
    let report = report_ops(source);
    assert!(
        rows(&report, "dead_code").is_empty(),
        "the dynamic operand's null check stays live: {:?}",
        report.diagnostics
    );
}

// —— the keep-honest is-test ————————————————————————————————————————————

/// A `dynamic` receiver against an undefined name never folds (the #2450
/// dynamic face, the corpus's dominant `x is T` shape): the spelling's
/// code reports and both branches stay live (probe p12_isdead).
#[test]
fn dynamic_receiver_undefined_test_keeps_both_arms() {
    let source = concat!(
        "void f(dynamic x) {\n",
        "  if (x is Missing) {\n",
        "    print('a');\n",
        "  } else {\n",
        "    print('b');\n",
        "  }\n",
        "}\n",
    );
    let report = report_ops(source);
    assert!(
        rows(&report, "dead_code").is_empty(),
        "the dynamic receiver keeps both arms live: {:?}",
        report.diagnostics
    );
    assert!(
        code_names(&report)
            .iter()
            .any(|name| *name == "type_test_with_undefined_name"),
        "the undefined spelling reports its own code: {:?}",
        report.diagnostics
    );
}
