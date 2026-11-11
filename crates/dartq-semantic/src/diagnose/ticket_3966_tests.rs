//! #3966 pins — the forward-referenced top-level `Never` getter (the
//! `null_shortening` census family's last row).
//!
//! `main` reads a top-level getter the unit declares AFTER it. The flow
//! recorder's top-level map fills only as declarations resolve, so every
//! executable replay resolved the name as unresolved: the read face stayed
//! the nullable-unknown marker, the null-aware index-write chain never died
//! on the argument, and the oracle's `Dead code.` row on the write's
//! right-hand side never fired. That is the census residual
//! (`features/_core/null_aware/language/null_aware/null_shortening_test`):
//! the family's six oracle rows (80:23, 90:25, 101:27, 130:26, 140:28,
//! 151:30) all anchor the RHS `0` of a `?[]=` chain whose index argument
//! reads the forward getter (`Never get unreachable`, line 172, declared
//! after `main`).
//!
//! The listener now consults the collector's whole-unit index at every
//! container-less bare read and stages the declared face on the recorder's
//! unresolved-read channel (the recorder's own resolution still wins for
//! backward references, whose rows it already captured); a bottom face
//! additionally drains the read exit the official fires in
//! `recordStaticType` (`handleExit` on `type.isBottom`, ast.dart:11146-
//! 11153) — the flow's unresolved-read arm carries no exit of its own.
//! The declared face is a whole-unit fact (the official resolves elements
//! for the full unit before analysis), so the forward and backward twins
//! answer identically.
//!
//! Oracle provenance: `~/.dartq-oracle/3.13.7/dart-sdk/bin/dart analyze
//! --format=machine` — exactly one `DEAD_CODE` row per write fixture,
//! anchored at the RHS `0` (line 9 column 21, lexeme width 1); the
//! write-target, named-face, and backward fixtures probe the same way.

use super::test_rig::{assert_silent, report_ops, report_ops_bare, rows};
use crate::diagnose::FileReport;
use dartq_diagnostics::Diagnostic;

/// The byte offset of the write chain's RHS `0` inside `source` — the
/// anchor the oracle reports (`line:col` of the lexeme itself, width 1).
fn rhs_zero_offset(source: &str) -> u32 {
    let eq = source
        .find("= 0;")
        .expect("fixture must carry the `= 0;` write");
    (eq + "= ".len()) as u32
}

fn dead_code_rows(report: &FileReport) -> Vec<&Diagnostic> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "dead_code")
        .collect()
}

/// The family face: the write's `0` is dead on both branches (the null
/// branch skips the whole chain, the non-null branch dies reading the
/// forward `Never` getter), so the oracle reports one `Dead code.` row on
/// the RHS lexeme. The argument read and the method-argument shape stay
/// live (the read itself is evaluated on the non-null path — no row).
#[test]
fn forward_never_getter_kills_the_index_write() {
    let source = r#"class C {
  C operator [](Object? i) => this;
  void operator []=(Object? i, Object? v) {}
  void method(Object? v) {}
}

void main() {
  C? c = null;
  c?[unreachable] = 0;
  c?.method(unreachable);
  c?[unreachable];
}

Never get unreachable => throw 'x';
"#;
    let report = report_ops_bare(source);
    let hits = dead_code_rows(&report);
    assert_eq!(hits.len(), 1, "{:#?}", rows(&report));
    assert_eq!(
        hits[0].range.offset,
        rhs_zero_offset(source),
        "{:#?}",
        rows(&report)
    );
    assert_eq!(hits[0].range.length, 1, "{:#?}", rows(&report));
    assert_eq!(hits[0].message, "Dead code.", "{:#?}", rows(&report));
}

/// The backward twin (getter declared before `main`): the recorder's own
/// resolved-Top arm answers it — the same single row at the same anchor.
/// The staged channel must not shift the backward face (its row is ignored
/// when the recorder's own resolution hits).
#[test]
fn backward_never_getter_twin_converges_identically() {
    let source = r#"class C {
  C operator [](Object? i) => this;
  void operator []=(Object? i, Object? v) {}
}

Never get unreachable => throw 'x';

void main() {
  C? c = null;
  c?[unreachable] = 0;
}
"#;
    let report = report_ops_bare(source);
    let hits = dead_code_rows(&report);
    assert_eq!(hits.len(), 1, "{:#?}", rows(&report));
    assert_eq!(
        hits[0].range.offset,
        rhs_zero_offset(source),
        "{:#?}",
        rows(&report)
    );
    assert_eq!(hits[0].message, "Dead code.", "{:#?}", rows(&report));
}

/// The write-target gate: a plain assignment to the forward getter is a
/// write, not a read — the official resolves no read face on the target
/// (its rows are the setter-missing error and the chain's own row, nothing
/// after), so the exit and the staged face both skip the target. The
/// fixture keeps the write as the LAST statement: a write to a
/// Never-faced top-level getter that is FOLLOWED by another statement
/// opens a dead window from the write's right operand in the current tree
/// — a pre-existing face (it reproduces on a build without this lane's
/// change; the oracle stays silent) owned by the resolution layer's
/// write-target face, which answers the getter's spelling for a plain `=`
/// target instead of the setter's. That residual is out of this lane's
/// scope; this pin freezes the oracle-matching dead_code set of the
/// unpoisoned shape.
#[test]
fn forward_never_write_target_exits_nothing() {
    let source = r#"class C {
  C operator [](Object? i) => this;
  void operator []=(Object? i, Object? v) {}
}

void main() {
  C? c = null;
  c?[unreachable] = 0;
  unreachable = 3;
}

Never get unreachable => throw 'x';
"#;
    let report = report_ops_bare(source);
    let hits = dead_code_rows(&report);
    assert_eq!(hits.len(), 1, "{:#?}", rows(&report));
    assert_eq!(
        hits[0].range.offset,
        rhs_zero_offset(source),
        "{:#?}",
        rows(&report)
    );
    assert_eq!(hits[0].message, "Dead code.", "{:#?}", rows(&report));
}

/// The named-face staging must not manufacture rows: a forward-referenced
/// `int` getter read as the index operand (and as a discarded statement)
/// keeps the chain alive — the oracle is silent. The staged declared face
/// replaces the nullable-unknown marker, which must stay inert for
/// reachability (no exit on a non-bottom face).
#[test]
fn forward_named_getter_read_stays_live() {
    let source = r#"class C {
  C operator [](Object? i) => this;
  void operator []=(Object? i, Object? v) {}
}

void main() {
  C? c = null;
  c?[nonNever] = 0;
  nonNever;
}

int get nonNever => 1;
"#;
    let report = report_ops(source);
    assert_silent(&report);
}
