//! #3476 pins — nameless `LOCAL` anchors at the enclosing executable name.
//!
//! Oracle rule (upstream Dart SDK source,
//! `pkg/analyzer/lib/src/error/unused_local_elements_verifier.dart`,
//! `_reportDiagnosticForElement` :1204-1211): a nameless element reports at
//! `fragment.nameOffset ?? enclosingFragment?.nameOffset ?? 0` with
//! `length = fragment.name?.length ?? 0`. The nameless element's own
//! fragment carries `nameOffset == null`, so the anchor is the ENCLOSING
//! executable fragment's `nameOffset`, and the length is always 0 — one
//! set element per enclosing executable, whatever the recovered
//! declaration's own token range was.
//!
//! Landed here (this ticket's owning lane): `close_frame_locals`
//! (`engine_core.rs`) computes the innermost enclosing Executable frame's
//! `ret.name_span` and re-anchors every nameless declaration it emits —
//! the popped frame itself when the body's locals sit on it, else the
//! nearest enclosing Executable past the transparent Block frames. An
//! anonymous closure (`FunctionExpressionFragmentImpl.nameOffset == null`)
//! or a nameless executable contributes no `nameOffset` — its nameless
//! locals fall to the `?? 0` arm and anchor at the file start.

use super::test_rig::{CORE_PRELUDE, report_ops, rows};

/// `report_ops` mounts [`CORE_PRELUDE`] ahead of the fixture — every
/// expected offset shifts by its length.
fn at(source: &str, needle: &str) -> u32 {
    (CORE_PRELUDE.len() + source.find(needle).unwrap()) as u32
}

fn unused_local_rows(report: &super::FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "unused_local_variable")
        .map(|d| format!("@{}+{}: {}", d.range.offset, d.range.length, d.message))
        .collect()
}

/// The discriminating anchor: the nameless element reports at the
/// executable's NAME (the `f` identifier), not at the declaration keyword
/// or the recovered token — and carries zero length.
///
/// Oracle (`int f() { @ assert (true); return 0; }`, recovered `var`/`final`
/// group): `unused_local_variable '<unnamed>'` at `f`'s name offset, length
/// 0 — column 5, not column 1.
#[test]
fn nameless_local_anchors_at_function_name_not_decl() {
    let source = "int f() { var {} return 0; }\n";
    let report = report_ops(source);
    let f_at = at(source, "f");
    let hit = report.diagnostics.iter().any(|d| {
        d.code.name == "unused_local_variable"
            && d.range.offset == f_at
            && d.range.length == 0
            && d.message.contains("<unnamed>")
    });
    assert!(
        hit,
        "expected <unnamed> row anchored at f ({f_at}+0): {:#?}",
        rows(&report)
    );
    // The discriminating edge: no row may sit at the declaration keyword.
    let kw_at = at(source, "var");
    let stray = report
        .diagnostics
        .iter()
        .any(|d| d.code.name == "unused_local_variable" && d.range.offset == kw_at);
    assert!(
        !stray,
        "nameless row must not anchor at the declaration keyword ({kw_at}): {:#?}",
        unused_local_rows(&report)
    );
}

/// Every nameless recovered declaration inside one executable anchors at
/// the SAME (offset, 0) site and collapses to ONE row upstream — the
/// listener's `Set<Diagnostic>` dedup makes the 8 identical rows of a
/// `main() { final ((n0,) && r0); … }` recovery a single element.
///
/// Oracle: ONE `unused_local_variable '<unnamed>'` at `main`'s name, never
/// two.
#[test]
fn nameless_locals_in_one_function_collapse_to_one_row() {
    let source = "main() {\n  var {};\n  var {};\n}\n";
    let report = report_ops(source);
    let main_at = at(source, "main");
    let hits = report
        .diagnostics
        .iter()
        .filter(|d| {
            d.code.name == "unused_local_variable"
                && d.range.offset == main_at
                && d.range.length == 0
                && d.message.contains("<unnamed>")
        })
        .count();
    assert_eq!(
        hits,
        1,
        "expected ONE collapsed <unnamed> row at main ({main_at}+0): {:#?}",
        unused_local_rows(&report)
    );
}

/// A nameless local inside a LOCAL FUNCTION anchors at that local
/// function's own name — the innermost enclosing Executable fragment.
///
/// Oracle (`main() { void inner() { var {} } inner(); }`): the recovered
/// local's `enclosingFragment` is `inner`'s function fragment, so
/// `unused_local_variable '<unnamed>'` anchors at `inner`'s name offset,
/// length 0 — not at `main`'s.
#[test]
fn nameless_local_in_local_function_anchors_at_local_fn_name() {
    let source = "main() {\n  void inner() {\n    var {};\n  }\n  inner();\n}\n";
    let report = report_ops(source);
    let inner_at = at(source, "inner");
    let hit = report.diagnostics.iter().any(|d| {
        d.code.name == "unused_local_variable"
            && d.range.offset == inner_at
            && d.range.length == 0
            && d.message.contains("<unnamed>")
    });
    assert!(
        hit,
        "expected <unnamed> row anchored at inner ({inner_at}+0): {:#?}",
        rows(&report)
    );
}

/// A nameless element inside a class constructor anchors at the
/// constructor/class name — the constructor fragment's `nameOffset`.
///
/// Oracle (`class C { C() { var {} } }`): the recovered local's
/// `enclosingFragment` is the `C()` constructor fragment, whose
/// `nameOffset` is the `C` head — `unused_local_variable '<unnamed>'`
/// anchors there, length 0.
#[test]
fn nameless_local_in_constructor_anchors_at_ctor_name() {
    let source = "class C {\n  C() {\n    var {};\n  }\n}\n";
    let report = report_ops(source);
    let ctor_at = at(source, "C()");
    let hit = report.diagnostics.iter().any(|d| {
        d.code.name == "unused_local_variable"
            && d.range.offset == ctor_at
            && d.range.length == 0
            && d.message.contains("<unnamed>")
    });
    assert!(
        hit,
        "expected <unnamed> row anchored at C ({ctor_at}+0): {:#?}",
        rows(&report)
    );
}

/// Negative control: a NAMED unused local still anchors at its own
/// declaration token — the re-anchor touches only the nameless recovery
/// face, never a name the parser produced.
///
/// Oracle (`void m() { var foo = 1; }`): `unused_local_variable` at `foo`,
/// length 3.
#[test]
fn named_local_still_anchors_at_own_decl() {
    let source = "class A {\n  void m() {\n    var foo = 1;\n  }\n}\n";
    let report = report_ops(source);
    let foo_at = at(source, "foo");
    let hit = report.diagnostics.iter().any(|d| {
        d.code.name == "unused_local_variable"
            && d.range.offset == foo_at
            && d.range.length == 3
            && d.message.contains("'foo'")
    });
    assert!(
        hit,
        "expected named 'foo' row anchored at its own decl ({foo_at}+3): {:#?}",
        rows(&report)
    );
}
