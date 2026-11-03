//! Ticket 3842 — const-fault operator extents and wildcard reads.
//!
//! Every expectation here was read off the pinned oracle
//! (`dart analyze`, 3.13.7) for the same source, so both directions of each
//! face are pinned: the shapes that must report and the shapes that must
//! stay silent.
//!
//! Two decision-order rules are under test:
//!
//! 1. **A const-eval fault anchors at its operator node.** Upstream's
//!    `ConstantVisitor.visitUnaryExpression` hands the whole
//!    `PrefixExpression` to `DartObjectComputer`, whose `_reportError`
//!    reports that node's extent (upstream Dart SDK source,
//!    pkg/analyzer/lib/src/dart/constant/evaluation.dart); the capture folds
//!    a prefix operator onto its operand without widening the node, so the
//!    emit site re-reads the operator (`Engine::unary_fault_extent`). A
//!    *non-constancy* is NOT widened: it anchors at the failing leaf.
//!
//! 2. **A wildcard read is undefined only when nothing declares it.** `_`
//!    names no const variable, but with a `_`-named type in scope the read
//!    folds to that type literal — a valid constant, so no const row
//!    (`const y = _;` after `class _ {}` is silent upstream), while the
//!    undeclared spelling pairs `undefined_identifier` with the const site
//!    default.

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

/// The one-library rig (`const_eval_faces::face_tests`'s): `source` is the
/// sole analyzed file of `package:t`, no imports.
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

/// The `(offset, length)` of the `ordinal`-th occurrence of `needle` — the
/// expectation source for every span pinned below (the fixtures are ASCII,
/// so byte and UTF-16 offsets agree).
fn at(source: &str, needle: &str, ordinal: usize) -> (u32, u32) {
    let (index, _) = source
        .match_indices(needle)
        .nth(ordinal)
        .unwrap_or_else(|| panic!("{needle:?} #{ordinal} not in {source:?}"));
    (index as u32, needle.len() as u32)
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

/// `!`/`-`/`~` over a null-valued const: the throws row covers the whole
/// unary expression, not the operand the capture folded it onto (oracle:
/// `!D` → 2 columns starting at the operator).
#[test]
fn ticket_3842_unary_throws_extent() {
    let src = "const dynamic D = null;\n\
               const c1 = !D;\n\
               const c2 = -D;\n\
               const c3 = ~D;\n";
    let rep = unit_report(src);
    let rows = const_rows(&rep);
    assert_eq!(
        rows,
        [
            ("const_eval_throws_exception", at(src, "!D", 0).0, 2),
            ("const_eval_throws_exception", at(src, "-D", 0).0, 2),
            ("const_eval_throws_exception", at(src, "~D", 0).0, 2),
        ],
        "{rows:?}"
    );
}

/// A nested prefix anchors at the innermost operator node that threw
/// (oracle: `!!D` → the inner `!D`; `- - D` → `- D`), and the trivia
/// between an operator and its operand stays inside the extent.
#[test]
fn ticket_3842_nested_unary_extent() {
    let src = "const dynamic D = null;\n\
               const a = !!D;\n\
               const b = - - D;\n";
    let rep = unit_report(src);
    let rows = const_rows(&rep);
    assert_eq!(
        rows,
        [
            // `!D` matches at the second `!` (the inner node).
            ("const_eval_throws_exception", at(src, "!D", 0).0, 2),
            ("const_eval_throws_exception", at(src, "- D", 0).0, 3),
        ],
        "{rows:?}"
    );
}

/// The operand-type faults widen too — upstream hands the same
/// `PrefixExpression` to the computer, so the entity is the operator's
/// (oracle: `!1` → `!1`, `~'a'` → `~'a'`, `-'a'` → `-'a'`).
#[test]
fn ticket_3842_unary_type_fault_extent() {
    let src = "const a = !1;\n\
               const b = ~'a';\n\
               const c = -'a';\n";
    let rep = unit_report(src);
    let rows = const_rows(&rep);
    assert_eq!(
        rows,
        [
            ("const_eval_type_bool", at(src, "!1", 0).0, 2),
            ("const_eval_type_int", at(src, "~'a'", 0).0, 4),
            ("const_eval_type_num", at(src, "-'a'", 0).0, 4),
        ],
        "{rows:?}"
    );
}

/// A binary operator the *binary* grammar produced keeps its own extent: a
/// fault carried by the left operand does not swallow the operator (oracle:
/// `1 - 'a'` reports the whole binary, `-'a' * 2` reports the unary alone).
/// The widening probe rejects a candidate operator with a value-ending byte
/// on its left, which is what a binary operator always has.
#[test]
fn ticket_3842_binary_extent_not_widened() {
    let src = "const a = 1 - 'a';\n\
               const b = -'a' * 2;\n";
    let rep = unit_report(src);
    let rows = const_rows(&rep);
    assert_eq!(
        rows,
        [
            ("const_eval_type_num", at(src, "1 - 'a'", 0).0, 7),
            // The unary node's fault propagates through the binary, so the
            // row is the unary's — never the enclosing product.
            ("const_eval_type_num", at(src, "-'a'", 0).0, 4),
        ],
        "{rows:?}"
    );
}

/// The other direction of the same decision: a *non-constancy* anchors at
/// the failing leaf, so the widening must not reach it (oracle: `!b` over a
/// mutable `b` reports the 1-column `b`).
#[test]
fn ticket_3842_non_constant_stays_at_leaf() {
    let src = "bool b = false;\n\
               const c = !b;\n";
    let rep = unit_report(src);
    let rows = const_rows(&rep);
    assert_eq!(
        rows,
        [(
            "const_initialized_with_non_constant_value",
            at(src, "!b", 0).0 + 1,
            1
        )],
        "{rows:?}"
    );
}

/// A wildcard read with a `_`-named type in scope folds to that type
/// literal — a valid constant, so the const faces stay silent (oracle:
/// silent for `class _ {}` + `const y = _;`).
#[test]
fn ticket_3842_wildcard_declared_is_constant() {
    let src = "class _ {\n\
               const _(int x);\n\
               }\n\
               const y = _;\n";
    let rep = unit_report(src);
    let rows = const_rows(&rep);
    assert!(rows.is_empty(), "{rows:?}");
}

/// The negative direction: with nothing named `_` in scope the read is
/// undefined and the const site default fires beside `undefined_identifier`
/// (oracle: `const y = _;` reports both rows at the 1-column `_`).
#[test]
fn ticket_3842_wildcard_undeclared_reports() {
    let src = "const y = _;\n";
    let rep = unit_report(src);
    let rows = const_rows(&rep);
    assert_eq!(
        rows,
        [(
            "const_initialized_with_non_constant_value",
            at(src, "_", 0).0,
            1
        )],
        "{rows:?}"
    );
}
