//! #3946 pins — the nested-literal context channel: a function literal
//! inside an outer literal's `=>` body must bind the outer context's
//! INSTANTIATED return face, never the declaration's signature re-read.
//!
//! The convicted fixture is the old-style generic typedef
//! `typedef List<X> Function(int) F<X>()`: `F<int> g = () => (int i) =>
//! <int>[];` opens the OUTER `() =>` under `F<int>`'s folded
//! `List<int> Function(int) Function()` — correct — but the INNER
//! `(int i) => <int>[]` opened at the SAME row mark (the outer literal's
//! deferred rows never advanced the count), so the declaration-initializer
//! channel claimed it a second time and re-bound the whole
//! `List<int> Function(int) Function()` signature. The inner body's
//! `<int>[]` then read `List<int> Function(int)` as its required return
//! and the invented `return_of_invalid_type_from_closure` row fired.
//!
//! The fix reorders `closure_context_ty`: the enclosing-executable
//! channels (return position / arrow-body position) now run BEFORE the
//! declaration-initializer channel — upstream `inferLocalFunction` reads
//! the literal's own AST position, never the enclosing declaration's, so
//! a literal nested in another literal's body infers under the enclosing
//! context's return (the instantiated face), not the written type. The
//! `=>`-body gate now also admits the enclosing literal's own frame
//! (`ret: None` + unbraced body — the literal's own frame is skipped).
//!
//! Oracle provenance: `~/.dartq-oracle/3.13.7/dart-sdk/bin/dart analyze
//! --format=machine` — silent on the `<int>[]` nested fixtures (case1,
//! case5, case6, case7, case10), `RETURN_OF_INVALID_TYPE_FROM_CLOSURE`
//! `'List<String>' isn't returnable from a 'List<int>' function` on the
//! mismatched twins (case3, case8), the declared-return pair
//! `RETURN_OF_INVALID_TYPE` + `RETURN_OF_INVALID_TYPE_FROM_CLOSURE` on
//! the `return () => (int i) => <String>[]` fixture (case9).

use super::test_rig::{code_names, report_ops};

/// The convicted case: `F<int> g = () => (int i) => <int>[];` — the
/// inner literal binds `List<int> Function(int)` (the instantiated
/// context return) and `<int>[]` meets `List<int>` exactly. The pre-fix
/// engine re-bound the declaration's whole signature and invented the
/// `return_of_invalid_type_from_closure` row.
#[test]
fn nested_literal_binds_the_instantiated_context_return() {
    let report = report_ops(
        "typedef List<X> Function(int) F<X>();\n\
         void main() {\n  \
         F<int> g = () => (int i) => <int>[];\n\
         g.toString();\n\
         }",
    );
    assert!(
        report
            .diagnostics
            .iter()
            .all(|d| d.code.name == "unused_local_variable"),
        "{:?}",
        code_names(&report)
    );
}

/// The mismatched twin keeps reporting — against the INSTANTIATED face:
/// the message spells `'List<String>'` against the `'List<int>'`
/// context return, never the raw `'List<X>'` typedef parameter.
#[test]
fn nested_literal_reports_against_the_instantiated_return() {
    let report = report_ops(
        "typedef List<X> Function(int) F<X>();\n\
         void main() {\n  \
         F<int> g = () => (int i) => <String>[];\n\
         g.toString();\n\
         }",
    );
    let hits: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "return_of_invalid_type_from_closure")
        .collect();
    assert_eq!(hits.len(), 1, "{:?}", code_names(&report));
    assert!(
        hits[0].message.contains("'List<String>'") && hits[0].message.contains("'List<int>'"),
        "the instantiated faces must render, got {:?}",
        hits[0].message
    );
    assert!(
        !hits[0].message.contains("'List<X>'"),
        "the raw typedef parameter leaked: {:?}",
        hits[0].message
    );
}

/// A block-bodied INNER literal checks its `return` against the
/// instantiated context return too: `() => (int i) { return <int>[]; }`
/// stays silent (the `=>`-nested twin of the same channel).
#[test]
fn block_bodied_inner_checks_the_instantiated_return() {
    let report = report_ops(
        "typedef List<X> Function(int) F<X>();\n\
         void main() {\n  \
         F<int> g = () => (int i) { return <int>[]; };\n\
         g.toString();\n\
         }",
    );
    assert!(
        report
            .diagnostics
            .iter()
            .all(|d| d.code.name == "unused_local_variable"),
        "{:?}",
        code_names(&report)
    );
}

/// The block-bodied inner's mismatch reports the instantiated face —
/// `'List<String>'` against `'List<int>'`.
#[test]
fn block_bodied_inner_reports_against_the_instantiated_return() {
    let report = report_ops(
        "typedef List<X> Function(int) F<X>();\n\
         void main() {\n  \
         F<int> g = () => (int i) { return <String>[]; };\n\
         g.toString();\n\
         }",
    );
    let hits: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "return_of_invalid_type_from_closure")
        .collect();
    assert_eq!(hits.len(), 1, "{:?}", code_names(&report));
    assert!(
        hits[0].message.contains("'List<String>'") && hits[0].message.contains("'List<int>'"),
        "{:?}",
        hits[0].message
    );
}

/// Doubly nested: `() => () => (int i) => <int>[]` under a declared
/// `F<int> Function()` — each literal reads its enclosing context's
/// return, never re-reads the declaration.
#[test]
fn doubly_nested_literal_walks_the_context_returns() {
    let report = report_ops(
        "typedef List<X> Function(int) F<X>();\n\
         void main() {\n  \
         F<int> Function() h = () => () => (int i) => <int>[];\n\
         h.toString();\n\
         }",
    );
    assert!(
        report
            .diagnostics
            .iter()
            .all(|d| d.code.name == "unused_local_variable"),
        "{:?}",
        code_names(&report)
    );
}

/// The declared `=>` host arm: `F<int> f() => () => (int i) => <int>[]`
/// — the first literal reads `f`'s declared return (channel iv's
/// `ret: Some` arm), the second reads the first's context return.
#[test]
fn declared_arrow_body_still_feeds_the_nested_literal() {
    let report = report_ops(
        "typedef List<X> Function(int) F<X>();\n\
         F<int> f() => () => (int i) => <int>[];\n",
    );
    assert!(report.diagnostics.is_empty(), "{:?}", code_names(&report));
}

/// The outer literal's BLOCK body `return` already worked — keep it
/// green under the reorder: `() { return (int i) => <int>[]; }` binds
/// the inner literal under the outer context's return.
#[test]
fn outer_block_return_still_binds_the_inner_literal() {
    let report = report_ops(
        "typedef List<X> Function(int) F<X>();\n\
         void main() {\n  \
         F<int> g = () { return (int i) => <int>[]; };\n\
         g.toString();\n\
         }",
    );
    assert!(
        report
            .diagnostics
            .iter()
            .all(|d| d.code.name == "unused_local_variable"),
        "{:?}",
        code_names(&report)
    );
}

/// A non-typedef control: `var h = (x) => 3;` inside the same body —
/// the reorder leaves the no-context literal silent.
#[test]
fn plain_nested_literal_keeps_the_no_context_posture() {
    let report = report_ops(
        "typedef List<X> Function(int) F<X>();\n\
         void main() {\n  \
         F<int> g = () => (int i) => <int>[];\n  \
         var h = (x) => 3;\n  \
         h.toString();\n\
         }",
    );
    assert!(
        report
            .diagnostics
            .iter()
            .all(|d| d.code.name == "unused_local_variable"),
        "{:?}",
        code_names(&report)
    );
}
