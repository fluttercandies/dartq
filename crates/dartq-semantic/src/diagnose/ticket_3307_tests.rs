//! The #3307 pins (`const_initialized_with_non_constant_value`, the LAZY
//! construct faces): the site verdict's anchor follows
//! `ConstantVisitor.visitBinaryExpression` / `visitConditionalExpression`
//! (upstream Dart SDK source, pkg/analyzer/lib/src/dart/constant/
//! evaluation.dart:674-849) rather than the runtime value.
//!
//! Two shapes alternate, and the corpus's 138 failing cases were dominated
//! by their absence:
//!
//! - The left operand SETTLES the value (`false && e`, `true || e`,
//!   non-null `?? e`, and a known condition's skipped branch). Upstream
//!   still runs `_reportNotPotentialConstants` over the operand it skips
//!   (:2237-2249): the first node the const grammar can never value anchors
//!   the site default, at that node.
//! - The right operand DECIDES the value (`true && e`, `false || e`). The
//!   operand is computed inside the lazy state, where a failure throws and
//!   `DartObjectComputer.lazyAnd`/`lazyOr` catch it into
//!   `InvalidConstant.forEntity(entity: node)` (:2293-2298) — the site
//!   default anchors at the whole binary, not at the failing operand.
//!
//! `lazyQuestionQuestion` is the one arm that does NOT re-anchor: a `null`
//! left hands the right operand's own verdict through untouched
//! (:2525-2532), so `null ?? a` keeps the operand's anchor.

use super::*;
use crate::collector::collect;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_resolution::LibraryGraphBuilder;
use dartq_scanner::ScanConfig;
use dartq_source::FileTable;
use std::path::Path;

/// Diagnoses `source` as the sole file of a library with no imports — the
/// sibling ticket rig mirrored verbatim.
fn report(source: &str) -> FileReport {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
    let (scanned, features, set) = scan(source, file);
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

/// `(code, source text at the row)` pairs — the span assertions key on the
/// written text so the anchors stay readable.
fn rows<'a>(report: &FileReport, source: &'a str) -> Vec<(&'static str, &'a str)> {
    report
        .diagnostics
        .iter()
        .map(|d| {
            (
                d.code.name,
                &source[d.range.offset as usize..d.range.end() as usize],
            )
        })
        .collect()
}

fn count(rows: &[(&'static str, &str)], code: &str) -> usize {
    rows.iter().filter(|(c, _)| *c == code).count()
}

const SITE: &str = "const_initialized_with_non_constant_value";

// —— the left operand settles the value: the skipped right is scanned ——

/// `false && a` over a non-const `a`: the value is already false, so the
/// skipped operand is scanned and the site default anchors at `a`
/// (evaluation.dart:708-714).
#[test]
fn and_left_settles_value_anchors_skipped_operand() {
    let source = "final a = false;\nconst c = false && a;\n";
    let rep = report(source);
    let rows = rows(&rep, source);
    assert!(rows.contains(&(SITE, "a")), "{rows:?}");
    assert_eq!(count(&rows, SITE), 1, "{rows:?}");
}

/// `true || a`: same arm on `||` (evaluation.dart:716-722).
#[test]
fn or_left_settles_value_anchors_skipped_operand() {
    let source = "final a = false;\nconst c = true || a;\n";
    let rep = report(source);
    let rows = rows(&rep, source);
    assert!(rows.contains(&(SITE, "a")), "{rows:?}");
    assert_eq!(count(&rows, SITE), 1, "{rows:?}");
}

/// `0 ?? a`: the left is non-null, so `a` is skipped and scanned
/// (evaluation.dart:724-730).
#[test]
fn if_null_left_settles_value_anchors_skipped_operand() {
    let source = "final a = 1;\nconst c = 0 ?? a;\n";
    let rep = report(source);
    let rows = rows(&rep, source);
    assert!(rows.contains(&(SITE, "a")), "{rows:?}");
    assert_eq!(count(&rows, SITE), 1, "{rows:?}");
}

/// `true ? 1 : a`: a known condition makes the `else` the skipped branch,
/// scanned before the taken branch is evaluated (evaluation.dart:825-831).
#[test]
fn conditional_skipped_else_is_scanned() {
    let source = "final a = 1;\nconst c = true ? 1 : a;\n";
    let rep = report(source);
    let rows = rows(&rep, source);
    assert!(rows.contains(&(SITE, "a")), "{rows:?}");
    assert_eq!(count(&rows, SITE), 1, "{rows:?}");
}

/// `false ? a : 1`: the mirror arm, the `then` branch is the skipped one
/// (evaluation.dart:832-837).
#[test]
fn conditional_skipped_then_is_scanned() {
    let source = "final a = 1;\nconst c = false ? a : 1;\n";
    let rep = report(source);
    let rows = rows(&rep, source);
    assert!(rows.contains(&(SITE, "a")), "{rows:?}");
    assert_eq!(count(&rows, SITE), 1, "{rows:?}");
}

/// A skipped operand the const grammar CAN value keeps the silence: `false
/// && CONST` is a constant (`getNotPotentiallyConstants` collects nothing
/// for a const variable read).
#[test]
fn skipped_const_operand_stays_silent() {
    let source = "const a = false;\nconst c = false && a;\nconst d = true || a;\nconst e = 0 ?? a;\n";
    let rep = report(source);
    let rows = rows(&rep, source);
    assert_eq!(count(&rows, SITE), 0, "{rows:?}");
}

// —— the right operand decides the value: failures re-anchor at the node —

/// `true && a`: the operand decides the value, so the failure is computed
/// inside the lazy state and re-anchored at the whole binary
/// (evaluation.dart:2293-2298).
#[test]
fn and_right_decides_value_anchors_whole_binary() {
    let source = "final a = false;\nconst c = true && a;\n";
    let rep = report(source);
    let rows = rows(&rep, source);
    assert!(rows.contains(&(SITE, "true && a")), "{rows:?}");
    assert_eq!(count(&rows, SITE), 1, "{rows:?}");
}

/// `false || a`: the `||` twin of the same arm.
#[test]
fn or_right_decides_value_anchors_whole_binary() {
    let source = "final a = false;\nconst c = false || a;\n";
    let rep = report(source);
    let rows = rows(&rep, source);
    assert!(rows.contains(&(SITE, "false || a")), "{rows:?}");
    assert_eq!(count(&rows, SITE), 1, "{rows:?}");
}

/// `null ?? a`: the one lazy arm that does NOT re-anchor — a null left
/// returns the right operand's own verdict, so the anchor stays on `a`
/// (evaluation.dart:2525-2532).
#[test]
fn if_null_null_left_keeps_operand_anchor() {
    let source = "final a = 1;\nconst c = null ?? a;\n";
    let rep = report(source);
    let rows = rows(&rep, source);
    assert!(rows.contains(&(SITE, "a")), "{rows:?}");
    assert_eq!(count(&rows, SITE), 1, "{rows:?}");
}

/// `true ? new C() : 0`: the taken branch is evaluated and its own verdict
/// anchors the site row (evaluation.dart:829).
#[test]
fn conditional_taken_branch_keeps_its_anchor() {
    let source = "class C {}\nconst c = true ? new C() : 0;\n";
    let rep = report(source);
    let rows = rows(&rep, source);
    assert!(rows.contains(&(SITE, "new C()")), "{rows:?}");
    assert_eq!(count(&rows, SITE), 1, "{rows:?}");
}

// —— silent negatives ——————————————————————————————————————————————

/// Every lazy construct over constants stays quiet: the arithmetic fold,
/// the const variable read and the decided short circuits are constants.
#[test]
fn lazy_constructs_over_constants_stay_silent() {
    let source = "const a = true;\nconst b = false;\n\
                  const c = a && b;\nconst d = a || b;\nconst e = 1 + 2;\n\
                  const f = null ?? 3;\nconst g = true ? 1 : 2;\n";
    let rep = report(source);
    let rows = rows(&rep, source);
    assert_eq!(count(&rows, SITE), 0, "{rows:?}");
}

/// A nested lazy construct inside a collection element keeps the element
/// face's own code and the declaration's site row at the nested failing
/// node — the re-anchor applies to the binary, not to the literal.
#[test]
fn lazy_inside_list_element_keeps_its_anchor() {
    let source = "final a = false;\nconst c = [true && a];\n";
    let rep = report(source);
    let rows = rows(&rep, source);
    assert!(rows.contains(&(SITE, "true && a")), "{rows:?}");
    assert_eq!(count(&rows, SITE), 1, "{rows:?}");
}
