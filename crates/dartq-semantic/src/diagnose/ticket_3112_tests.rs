//! #3112 — the `unnecessary_final` field/super-formal gate.
//!
//! The whole-unit pass used to report every `final` inside a parenthesis
//! (minus the `for`/`if-case` head exclusions); the oracle emits
//! `UNNECESSARY_FINAL` only from `visitFieldFormalParameter` /
//! `visitSuperFormalParameter` — `final this.x`/`final super.x` — and
//! suppresses it inside a primary-constructor declaration (upstream
//! Dart SDK source, pkg/analyzer/lib/src/error/
//! best_practices_verifier.dart `_checkFinalParameter` :1040-1049, the
//! call sites :496-504/:902-910, `_inPrimaryConstructorDeclaration`
//! :101/:818-824; `parsePrimaryConstructorOpt`,
//! pkg/_fe_analyzer_shared/lib/src/parser/parser_impl.dart:3782-3843).
//! Every other `final` inside a formal list answers the parser's
//! `extraneous_modifier` face, never this one — the pins mirror the
//! oracle `3.13.7` rows on each shape.

use super::test_rig::report_ops_bare;
use super::*;

fn rows_named<'d>(report: &'d FileReport, name: &str) -> Vec<&'d Diagnostic> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == name)
        .collect()
}

// —— fire arms: `final this.`/`final super.` ————————————————————————————

/// `final this.x` reports at the `final` keyword (upstream
/// `visitFieldFormalParameter` → `_checkFinalParameter`).
#[test]
fn uf_field_formal_fires() {
    let main = "class A {\n  final int x;\n  A(final this.x);\n}\n";
    let report = report_ops_bare(main);
    let rows = rows_named(&report, "unnecessary_final");
    assert_eq!(rows.len(), 1, "{report:?}");
    assert_eq!(rows[0].severity, Severity::Warning);
    let at = main.find("final this").expect("fixture anchor") as u32;
    assert_eq!(
        rows[0].range,
        SourceRange {
            offset: at,
            length: 5
        }
    );
}

/// `final super.x` reports at the `final` keyword (upstream
/// `visitSuperFormalParameter` → `_checkFinalParameter`).
#[test]
fn uf_super_formal_fires() {
    let main =
        "class A {\n  final int x;\n  A(this.x);\n}\nclass B extends A {\n  B(final super.x);\n}\n";
    let report = report_ops_bare(main);
    let rows = rows_named(&report, "unnecessary_final");
    assert_eq!(rows.len(), 1, "{report:?}");
    let at = main.find("final super").expect("fixture anchor") as u32;
    assert_eq!(
        rows[0].range,
        SourceRange {
            offset: at,
            length: 5
        }
    );
}

// —— silent arms: every other `final` inside a formal list ——————————————

/// A plain `final` formal is the parser's `extraneous_modifier` face,
/// never `unnecessary_final` — the oracle reports no WARNING row on
/// `void f(final int x) {}` (only the SYNTACTIC_ERROR).
#[test]
fn uf_plain_formal_silent() {
    let report = report_ops_bare("void f(final int x) {}\n");
    assert!(
        rows_named(&report, "unnecessary_final").is_empty(),
        "{report:?}"
    );
}

/// Named, optional, local-function, closure, and multi-`final` formal
/// lists stay silent on the WARNING face (the oracle emits
/// `extraneous_modifier` only).
#[test]
fn uf_other_formal_shapes_silent() {
    let main = "void g({final int? x}) {}\n\
                void h([final int? x]) {}\n\
                void m() {\n  (final int x) {};\n}\n\
                void n(final a, final b) {}\n";
    let report = report_ops_bare(main);
    assert!(
        rows_named(&report, "unnecessary_final").is_empty(),
        "{report:?}"
    );
}

/// A `for` head's `final` is a loop-variable declaration, not a formal
/// — still silent.
#[test]
fn uf_for_head_silent() {
    let report = report_ops_bare("void f() {\n  for (final y in []) {}\n}\n");
    assert!(
        rows_named(&report, "unnecessary_final").is_empty(),
        "{report:?}"
    );
}

// —— suppress arm: the primary-constructor declaration head ——————————————

/// `final this.x` inside a primary-constructor declaration head stays
/// silent — `_inPrimaryConstructorDeclaration`
/// (best_practices_verifier.dart:818-824); the parser face owns the
/// `initializing_declaring_parameter` error instead.
#[test]
fn uf_primary_ctor_suppressed() {
    let report = report_ops_bare("class Q(final this.x) {\n  final int x;\n}\n");
    assert!(
        rows_named(&report, "unnecessary_final").is_empty(),
        "{report:?}"
    );
}

/// The generic + named-constructor head spellings suppress the same way
/// (`class G<T>(…)`, `class H<T>.n(…)`, `enum E(…)`): the walk-back
/// crosses the optional `.name` and `<T…>` group to the declaration
/// keyword.
#[test]
fn uf_primary_ctor_generic_and_named_suppressed() {
    let main = "class G<T>(final this.x) {\n  final int x;\n}\n\
                class H<T>.n(final this.x) {\n  final int x;\n}\n\
                enum E(final this.x) {\n  v(1);\n  final int x;\n  const E(this.x);\n}\n";
    let report = report_ops_bare(main);
    assert!(
        rows_named(&report, "unnecessary_final").is_empty(),
        "{report:?}"
    );
}

/// A regular constructor inside the primary ctor's own BODY still fires
/// — the suppress covers the head's parameter list only, not the class
/// body (the `_inPrimaryConstructorDeclaration` flag clears when the
/// declaration node closes).
#[test]
fn uf_body_ctor_still_fires() {
    let main = "class Q(int x) {\n  Q.named(final this.x);\n}\n";
    let report = report_ops_bare(main);
    let rows = rows_named(&report, "unnecessary_final");
    assert_eq!(rows.len(), 1, "{report:?}");
    let at = main.find("final this").expect("fixture anchor") as u32;
    assert_eq!(
        rows[0].range,
        SourceRange {
            offset: at,
            length: 5
        }
    );
}
