//! The anchor-side `typedef` fold pins (#3743): a receiver head spelling a
//! type alias reaches the aliased type's member face (upstream Dart SDK
//! source, pkg/analyzer/lib/src/dart/element/element.dart — a
//! `TypeAliasElement` use resolves through its aliased type before any
//! member question), which retires the `undefined_getter`/`undefined_method`
//! rows the broken-chain fallback used to emit for `Alias.member` on a
//! value-typed receiver (`p.im()` over `P p`, `mkPublicClass().im()`). A
//! genuine miss still reports, and an alias cycle terminates instead of
//! re-anchoring forever.

use super::test_rig::{RigImport, report_ops, report_ops_imports};
use crate::diagnose::FileReport;

/// The `undefined_getter`/`undefined_method`/`undefined_setter` rows the
/// report carries — the codes the anchor fold decides.
fn undefined_rows(report: &FileReport) -> Vec<(&'static str, u32)> {
    report
        .diagnostics
        .iter()
        .filter(|d| {
            matches!(
                d.code.name,
                "undefined_getter" | "undefined_method" | "undefined_setter"
            )
        })
        .map(|d| (d.code.name, d.range.offset))
        .collect()
}

/// `typedef P = _P;` then `P().im()` / `P().g` on a created value and
/// `p.im()` on an alias-declared local: the member walk folds the alias
/// head to `_P`'s face, so no undefined row stands.
#[test]
fn typedef_anchor_reaches_the_aliased_class_face() {
    let source = "class _P {\n  void im() {}\n  int get g => 0;\n}\n\
                  typedef P = _P;\n\
                  void f() {\n  P().im();\n  P().g;\n  P p = P();\n  p.im();\n  p.g;\n}\n";
    let rows = undefined_rows(&report_ops(source));
    assert!(rows.is_empty(), "the aliased face answers: {rows:?}");
}

/// The cross-library twin: an exported alias names a PRIVATE class of its
/// own library (`typedef Pub = _Priv;` in `a.dart`), so the fold must
/// re-anchor in the alias's declaring scope — the use site's scope cannot
/// resolve `_Priv`.
#[test]
fn typedef_anchor_folds_a_cross_library_private_head() {
    let imported = "class _Priv {\n  void im() {}\n  int get g => 0;\n}\n\
                    typedef Pub = _Priv;\n";
    let main = "import 'package:t/a.dart';\n\
                void f() {\n  Pub().im();\n  Pub().g;\n  Pub p = Pub();\n  p.im();\n}\n";
    let rows = undefined_rows(&report_ops_imports(
        main,
        &[RigImport {
            uri: "package:t/a.dart",
            prefix: None,
            source: imported,
        }],
    ));
    assert!(rows.is_empty(), "the aliased face answers: {rows:?}");
}

/// The fold narrows resolution, never reports: `p.zzz` over a
/// value-typed alias receiver still reports — the aliased class declares
/// no `zzz`, so the genuine-miss direction is untouched.
#[test]
fn typedef_anchor_still_reports_a_genuine_miss() {
    let source = "class _P {\n  void im() {}\n}\n\
                  typedef P = _P;\n\
                  void f() {\n  P().zzz;\n}\n";
    let report = report_ops(source);
    let rows = undefined_rows(&report);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].0, "undefined_getter", "{rows:?}");
}

/// `typedef A = _C; typedef P = A` — the chain folds hop by hop, so the
/// terminal class's members answer on the value-typed receiver.
#[test]
fn typedef_anchor_chases_a_chain() {
    let source = "class _C {\n  void im() {}\n}\n\
                  typedef A = _C;\ntypedef P = A;\n\
                  void f() {\n  P().im();\n  P p = P();\n  p.im();\n}\n";
    let rows = undefined_rows(&report_ops(source));
    assert!(rows.is_empty(), "the chain answers: {rows:?}");
}

/// `typedef A = B; typedef B = A;` — the chase stops at the repeat
/// instead of re-anchoring forever (the pin is termination: the rig
/// returns at all).
#[test]
fn alias_cycle_terminates() {
    let source = "typedef A = B;\ntypedef B = A;\nvoid f(A a) {\n  a.x;\n}\n";
    let report = report_ops(source);
    assert!(
        report.diagnostics.len() < 8,
        "the cycle guard answers one verdict, got {:#?}",
        undefined_rows(&report)
    );
}
