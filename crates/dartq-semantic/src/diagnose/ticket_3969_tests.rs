//! Ticket 3969 — a nested prefix fault anchors at the INNERMOST operator
//! node, never at the enclosing unary's operator.
//!
//! Every expectation below was read off the pinned oracle (`dart analyze`
//! 3.13.7, `--format=machine`) for the same source, so the extents are
//! byte-exact:
//!
//! ```text
//! const dynamic D = null;
//! const a = !!D;      -> const_eval_throws_exception  2|12|2   (inner `!D`)
//! const b = - - D;    -> const_eval_throws_exception  3|13|3   (inner `- D`)
//! const dynamic E = 's';
//! const c = !E;       -> const_eval_type_bool         5|11|2
//! const d = -E;       -> const_eval_type_num          6|11|2
//! const e = ~E;       -> const_eval_type_int          7|11|2
//! const f = !!E;      -> const_eval_type_bool         8|12|2   (inner `!E`)
//! const g = - -E;     -> const_eval_type_num          9|13|2   (inner `-E`)
//! const h = ~ ~E;     -> const_eval_type_int         10|13|2   (inner `~E`)
//! ```
//!
//! Upstream's `ConstantVisitor.visitPrefixExpression` returns the operand's
//! non-`DartObjectImpl` verdict verbatim (upstream Dart SDK source,
//! pkg/analyzer/lib/src/dart/constant/evaluation.dart,
//! `visitPrefixExpression`), so the reported entity is the innermost
//! `PrefixExpression` that threw. The capture's `Expr::Unary` node span
//! already covers operator-through-operand (`unary_span`), and
//! `Engine::unary_fault_extent` must leave a span that already begins at
//! an operator byte untouched — re-widening swallowed the enclosing
//! `!`/`-` and reported `(34,3)`/`(49,5)` against the oracle's
//! `(35,2)`/`(51,3)`.

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

/// The one-library rig (`ticket_3842_tests`'s): `source` is the sole
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

/// The null-valued `dynamic` operand: each nested prefix throws inside the
/// inner application, so both rows anchor at the INNER operator node —
/// the exact regression shape the ticket was filed on (pre-fix dartq
/// emitted `(34,3)` over `!!D` and `(49,5)` over `- - D`; the oracle's
/// entities are `!D` at `(35,2)` and `- D` at `(51,3)`).
#[test]
fn ticket_3969_nested_unary_null_dynamic_inner_anchor() {
    let src = "const dynamic D = null;\n\
               const a = !!D;\n\
               const b = - - D;\n";
    let rep = unit_report(src);
    let rows = const_rows(&rep);
    assert_eq!(
        rows,
        [
            ("const_eval_throws_exception", 35, 2),
            ("const_eval_throws_exception", 51, 3),
        ],
        "{rows:?}"
    );
}

/// The operand-type faults nest identically: `!!E` over a String-valued
/// `dynamic` throws inside the inner `!E`, and the same holds for the
/// `- -`/ `~ ~` pairs — each row lands on the inner operator's own
/// 2-column extent.
#[test]
fn ticket_3969_nested_unary_type_fault_inner_anchor() {
    let src = "const dynamic E = 's';\n\
               const f = !!E;\n\
               const g = - -E;\n\
               const h = ~ ~E;\n";
    let rep = unit_report(src);
    let rows = const_rows(&rep);
    assert_eq!(
        rows,
        [
            ("const_eval_type_bool", 34, 2),
            ("const_eval_type_num", 50, 2),
            ("const_eval_type_int", 66, 2),
        ],
        "{rows:?}"
    );
}

/// The single-operator rows beside them keep their whole-operator extent —
/// the already-widened `Expr::Unary` span is the oracle entity, and the
/// walk-back must not fire on it either direction.
#[test]
fn ticket_3969_single_unary_extent_untouched() {
    let src = "const dynamic E = 's';\n\
               const c = !E;\n\
               const d = -E;\n\
               const e = ~E;\n";
    let rep = unit_report(src);
    let rows = const_rows(&rep);
    assert_eq!(
        rows,
        [
            ("const_eval_type_bool", 33, 2),
            ("const_eval_type_num", 47, 2),
            ("const_eval_type_int", 61, 2),
        ],
        "{rows:?}"
    );
}
