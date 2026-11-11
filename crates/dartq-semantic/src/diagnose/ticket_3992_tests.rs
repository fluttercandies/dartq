//! Ticket 3992 — a const constructor's `assert(identical(a, b))` /
//! `assert(!identical(a, b))` initializer is SILENT: `identical` is the
//! one call the potentially-constant grammar admits (upstream Dart SDK
//! source, `pkg/analyzer/lib/src/dart/constant/potentially_constant.dart`
//! — `_isPotentiallyConstant` answers true for a two-argument `identical`
//! invocation), and the assert's formals-bound operands are the
//! potentially-constant context's own names.
//!
//! Every expectation below was read off the pinned oracle (`dart analyze`
//! 3.13.7, `--format=machine`) for the same source — both fixtures probe
//! to zero diagnostic rows:
//!
//! ```text
//! class CheckIdentical {
//!   const CheckIdentical(Object? o1, Object? o2) : assert(identical(o1, o2));
//! }                                   -> (silent)
//! class CheckNotIdentical {
//!   const CheckNotIdentical(Object? o1, Object? o2) : assert(!identical(o1, o2));
//! }                                   -> (silent)
//! ```
//!
//! The regression: dartq reported `invalid_constant` ("Invalid constant
//! value.") at the whole `identical(o1, o2)` invocation — the co19
//! `Utils/expect_common.dart` `CheckIdentical`/`CheckNotIdentical`
//! declarations — because `init_invocation_level`'s `init_rows` arm
//! treats EVERY bare callee inside a const constructor's initializer
//! list as a `this`/`super` redirect head (`init_rows` is stamped for
//! all bare names there), so `identical(...)` folded to `Expr::Other`
//! through `end_init_invocation_arguments` instead of the generic
//! close's `Expr::Call` — the one fold the potentially-constant scan
//! admits.

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

/// Every `(code, offset, length)` row the report carries.
fn all_rows(report: &FileReport) -> Vec<(&'static str, u32, u32)> {
    report
        .diagnostics
        .iter()
        .map(|d| (d.code.name, d.range.offset, d.range.length))
        .collect()
}

/// The co19 `CheckIdentical` shape: the two-argument `identical` call is
/// the ONE invocation the potentially-constant grammar admits — the
/// oracle is silent on the whole declaration (probed against `dart
/// analyze` 3.13.7: zero rows).
///
/// The rig seeds no `dart:core` functions, so the callee row for
/// `identical` resolves as a bare miss and the resolution tier still
/// reports `undefined_method` — a fixture artifact the real SDK model
/// does not carry (`dartq analyze --sdk-path …` on the same source
/// reports only this ticket's regressed `invalid_constant`). The pin
/// asserts that row set exactly: the const face must fold through
/// `Expr::Call`, and the resolution artifact is what remains.
#[test]
fn ticket_3992_assert_identical_initializer_is_silent() {
    let src = "class CheckIdentical {\n\
               \x20 const CheckIdentical(Object? o1, Object? o2) : assert(identical(o1, o2));\n\
               }\n";
    let rep = unit_report(src);
    assert_eq!(all_rows(&rep), [("undefined_method", 79, 9)]);
}

/// The `CheckNotIdentical` twin: the `!` unary wraps the same admitted
/// call — `UnOp::Not` recurses into the operand like any subexpression,
/// so the silent verdict is identical.
#[test]
fn ticket_3992_assert_not_identical_initializer_is_silent() {
    let src = "class CheckNotIdentical {\n\
               \x20 const CheckNotIdentical(Object? o1, Object? o2) : assert(!identical(o1, o2));\n\
               }\n";
    let rep = unit_report(src);
    assert_eq!(all_rows(&rep), [("undefined_method", 86, 9)]);
}

/// The same admitted call in a FIELD initializer (`v = identical(o1, o2)`)
/// — the `init_rows` stamp covers every bare name in the list, not just
/// the assert's operands, so the exemption must not be assert-specific.
/// Oracle-probed: silent.
#[test]
fn ticket_3992_field_init_identical_is_silent() {
    let src = "class C {\n\
               \x20 final bool v;\n\
               \x20 const C(Object? o1, Object? o2) : v = identical(o1, o2);\n\
               }\n";
    let rep = unit_report(src);
    assert_eq!(all_rows(&rep), [("undefined_method", 66, 9)]);
}

/// The non-admitted call keeps the fold: `myIdentical(o1, o2)` is not
/// the potentially-constant grammar's `identical` — upstream reports
/// `invalid_constant` at the WHOLE invocation (the init-invocation
/// close's `Expr::Other` leaf, callee through the closing paren), so the
/// exemption must discriminate on the admitted shape, not "any call in
/// an initializer".
#[test]
fn ticket_3992_non_identical_call_still_reports_invalid_constant() {
    let src = "bool myIdentical(Object? a, Object? b) => a == b;\n\
               class C {\n\
               \x20 const C(Object? o1, Object? o2) : assert(myIdentical(o1, o2));\n\
               }\n";
    let rep = unit_report(src);
    let rows = all_rows(&rep);
    assert!(rows.contains(&("invalid_constant", 103, 19)), "{rows:?}");
}

/// The admitted shape's own guard: a wrong-arity `identical` call is NOT
/// the grammar's admitted form — upstream reports `invalid_constant` at
/// the whole invocation (beside the resolver's arity row), so the
/// exemption keys on `args.len() == 2`, never the callee spelling alone.
#[test]
fn ticket_3992_wrong_arity_identical_still_reports_invalid_constant() {
    let src = "class C {\n\
               \x20 const C(Object? o1) : assert(identical(o1));\n\
               }\n";
    let rep = unit_report(src);
    let rows = all_rows(&rep);
    assert!(rows.contains(&("invalid_constant", 41, 13)), "{rows:?}");
}

/// A NON-const literal inside the admitted call's arguments: upstream's
/// `_Collector._typedLiteral` collects `!node.isConst` at the LITERAL
/// (`[g]` — oracle 3.13.7 reports `invalid_constant` at 3:41, len 3), not
/// at the element.
#[test]
#[ignore = "divergent (probed 2026-11-05 vs oracle 3.13.7): dartq reports ZERO invalid_constant — the capture folds a bare `[g]` inside the admitted call's argument world to a deferring leaf, so collect_npc's Expr::Call recursion finds nothing to push. Owner: the const capture's non-const literal fold inside an admitted call's args — a `!isConst` TypedLiteral must surface as a non-constant leaf at the literal's own span"]
fn ticket_3992_literal_inside_identical_args_reports_at_literal() {
    let src = "int g = 0;\n\
               class C {\n\
               \x20 const C(Object? o) : assert(identical([g], o));\n\
               }\n";
    let rep = unit_report(src);
    let rows = all_rows(&rep);
    assert!(rows.contains(&("invalid_constant", 62, 3)), "{rows:?}");
}
