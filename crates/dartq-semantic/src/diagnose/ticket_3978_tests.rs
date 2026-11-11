//! Ticket 3978 — a MIXED run of prefix operators must be captured in the
//! syntactic wrap order: innermost operator first.
//!
//! The capture used to drain the pending prefix windows LIFO while the
//! listener latches them innermost-first (each window fires when its
//! operand completes), so `!-~x` was applied as `~(-(!x))` — a different
//! expression — and every level's extent landed on the wrong node. The
//! capture now pairs each window to its own operator token (descending
//! offset, `ConstCapture::wrap_unary_run`), which rebuilds the nesting
//! regardless of push order.
//!
//! Every expectation below was read off the pinned oracle (`dart analyze`
//! 3.13.7, `--format=machine`) for the same source:
//!
//! ```text
//! const y = 1 ~/ -~1;      -> (no diagnostics; -~1 == 2)
//! const dynamic D = null;
//! const a = ~-D;           -> const_eval_throws_exception  2|12|2  (inner `-D`)
//! const dynamic E = 's';
//! const b = -~E;           -> const_eval_type_int          2|12|2  (inner `~E`)
//! const c = !-~true;       -> const_eval_type_int          1|13|5  (inner `~true`)
//! ```
//!
//! Upstream's `ConstantVisitor.visitPrefixExpression` returns the
//! operand's non-`DartObjectImpl` verdict verbatim (upstream Dart SDK
//! source, pkg/analyzer/lib/src/dart/constant/evaluation.dart,
//! `visitPrefixExpression`), so the reported entity is the innermost
//! `PrefixExpression` that threw — which only the correctly nested tree
//! carries. The all-`!` shape `const x = !-~1;` is deliberately NOT
//! pinned: the oracle reports `const_eval_type_bool` at the whole
//! `!-~1` extent (1|11|4), the same row both capture orders produce
//! (the outer `!` faults on an int wherever it applies), so it cannot
//! discriminate the order.

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

/// The one-library rig (`ticket_3969_tests`'s): `source` is the sole
/// analyzed file of `package:t`, no imports.
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
    let core_file = files.intern(Path::new("<dart:core>"));
    builder.seed_implicit_core(
        lib,
        core_file,
        &["Object", "String", "int", "bool", "double", "num"],
    );
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

/// Every `(code, offset, length)` row whose code starts with `const_`.
fn const_rows(report: &FileReport) -> Vec<(&str, u32, u32)> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name.starts_with("const_"))
        .map(|d| (d.code.name, d.range.offset, d.range.length))
        .collect()
}

/// The VALUE pin: `1 ~/ -~1` is a valid constant (`~1 == -2`, negated
/// `2`, so `1 ~/ 2 == 0`) — the oracle reports nothing. The swapped
/// capture evaluates the right operand as `~(-(1)) == 0` and the
/// division by zero surfaces `const_eval_throws_idbze`: any const row
/// here is the capture-order regression.
#[test]
fn ticket_3978_mixed_run_evaluation_value() {
    let src = "const y = 1 ~/ -~1;\n";
    let rep = unit_report(src);
    let rows = const_rows(&rep);
    assert!(rows.is_empty(), "{rows:?}");
}

/// The EXTENT pin over a mixed run whose inner operator throws: `~-D`
/// faults inside the inner `-D` (negating null throws), and the verdict
/// propagates verbatim, so the oracle anchors at `-D` — the innermost
/// throwing `PrefixExpression` — never at the outer `~-D`. The swapped
/// capture built `-(~D)` instead and anchored at the outer extent
/// `(34,3)`.
#[test]
fn ticket_3978_mixed_run_extent_inner_throw() {
    let src = "const dynamic D = null;\n\
               const a = ~-D;\n";
    let rep = unit_report(src);
    let rows = const_rows(&rep);
    assert_eq!(rows, [("const_eval_throws_exception", 35, 2)], "{rows:?}");
}

/// The CODE pin over a mixed run: `-~E` over a String-valued `dynamic`
/// faults inside the inner `~E` (`const_eval_type_int` at `~E`). The
/// swapped capture applied `-` to the String first and reported
/// `const_eval_type_num` at the outer extent `(33,3)` — wrong code AND
/// wrong anchor.
#[test]
fn ticket_3978_mixed_run_type_fault_inner_anchor() {
    let src = "const dynamic E = 's';\n\
               const b = -~E;\n";
    let rep = unit_report(src);
    let rows = const_rows(&rep);
    assert_eq!(rows, [("const_eval_type_int", 34, 2)], "{rows:?}");
}

/// A three-deep mixed run: `!-~true` faults inside the innermost `~true`
/// (`const_eval_type_int`), which only the correctly nested tree carries
/// (`~true` spans five bytes). The swapped capture applied `!` to the
/// bool first — folding cleanly — and reported `const_eval_type_num`
/// for the `-` at the outer-ish extent `(11,6)`.
#[test]
fn ticket_3978_deep_mixed_run_type_fault_inner_anchor() {
    let src = "const c = !-~true;\n";
    let rep = unit_report(src);
    let rows = const_rows(&rep);
    assert_eq!(rows, [("const_eval_type_int", 12, 5)], "{rows:?}");
}
