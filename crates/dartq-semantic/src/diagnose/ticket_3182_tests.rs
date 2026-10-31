//! The #3182 Lane F `dead_code` misc pins — three small faces upstream of
//! `DeadCodeVerifier` (upstream Dart SDK source
//! pkg/analyzer/lib/src/error/dead_code_verifier.dart):
//!
//! 1. `visitFunctionDeclaration` (:72-82): an `_`-named LOCAL function is
//!    dead over its whole declaration — but only when the library's
//!    `wildcard-variables` feature is enabled
//!    (`_wildCardVariablesEnabled`; a `// @dart = 3.4` unit keeps `_` an
//!    ordinary name and stays silent).
//! 2. The Never-operand `as`-cast tail (oracle 3.13.1-probed): inside an
//!    initializer, `f() as int` with `f` returning `Never` reports
//!    `dead_code` anchored at the cast TYPE's head token — the operand's
//!    exit left the walk unreachable, so the type is the first dead node
//!    (`flowEnd` :253-279).
//! 3. `visitVariableDeclaration` (:118-134): a `late` LOCAL `_`-declared
//!    variable's initializer is `dead_code` over the initializer's own
//!    span — upstream's `deadCodeLateWildcardVariableInitializer` shares
//!    the `dead_code` name (`diagnostic.g.dart:3493`), same feature gate.

use crate::diagnose::test_rig::{code_names, report_ops, report_ops_wildcards, rows};

fn dead_ranges(source: &str) -> Vec<(usize, usize)> {
    let report = report_ops(source);
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "dead_code")
        .map(|d| {
            let offset = d.range.offset as usize - crate::diagnose::test_rig::CORE_PRELUDE.len();
            (offset, offset + d.range.length as usize)
        })
        .collect()
}

/// `void _() {}` inside a body — the whole declaration reports, from its
/// first token through the body's `}`.
#[test]
fn ticket_3182_wildcard_local_function_dead() {
    let source = "void f() {\n  void _() {}\n}";
    let dead = dead_ranges(source);
    let start = source.find("void _").unwrap();
    // `void _() {}` — the declaration spans `void` through its own `}`.
    let decl_end = start + source[start..].find('}').unwrap() + 1;
    assert!(
        dead.contains(&(start, decl_end)),
        "wildcard local fn decl row missing ({start}..{decl_end}): {dead:?}"
    );
}

/// A named local function and a `_` function expression both stay live —
/// the face reads `LocalFunctionElement` declarations only.
#[test]
fn ticket_3182_named_local_function_stays_live() {
    let source = "void f() {\n  void g() {}\n  var _ = () {};\n}";
    let dead = dead_ranges(source);
    assert!(
        dead.is_empty(),
        "named local fn / `_` fn-expression must not report: {dead:?}"
    );
}

/// Feature-off posture (`// @dart = 3.4`-shaped): `_` is an ordinary
/// name, the `_()` declaration is live.
#[test]
fn ticket_3182_wildcard_fn_silent_without_feature() {
    let report = report_ops_wildcards("void f() {\n  void _() {}\n}", false);
    assert!(
        !code_names(&report).iter().any(|name| *name == "dead_code"),
        "{:?}",
        rows(&report)
    );
}

/// `f() as int` inside an initializer with `f` returning `Never` — the
/// row anchors at the cast type's head token `int`, never at `f()` or the
/// whole cast.
#[test]
fn ticket_3182_never_operand_cast_reports_type_head() {
    let source = "Never f() => throw 0;\nvar x = f() as int;";
    let dead = dead_ranges(source);
    let head = source.find("int").unwrap();
    assert!(
        dead.contains(&(head, head + 3)),
        "cast-type head row missing ({head}..{}): {dead:?}",
        head + 3
    );
}

/// The same cast OUTSIDE an initializer reports the `int;` interval —
/// the operand's exit left the walk unreachable, so the cast type is the
/// first dead node and `flowEnd`'s statement trim runs it through the
/// `;` (dead_code_verifier.dart:253-279's outer-node walk), NOT the
/// bare head token the initializer face stages.
#[test]
fn ticket_3182_never_cast_outside_initializer_reports_statement() {
    let source = "Never f() => throw 0;\nvoid g() {\n  f() as int;\n}";
    let dead = dead_ranges(source);
    let head = source.rfind("int").unwrap();
    assert!(
        dead.contains(&(head, head + 4)),
        "`int;` dead-interval row missing ({head}..{}): {dead:?}",
        head + 4
    );
}

/// `late var _ = e` — the initializer `e` reports `dead_code` over its
/// own span, at any reachability.
#[test]
fn ticket_3182_late_wildcard_initializer_dead() {
    let source = "void f() {\n  late var _ = 0;\n}";
    let dead = dead_ranges(source);
    let init = source.find("= 0").unwrap() + 2;
    assert!(
        dead.contains(&(init, init + 1)),
        "late-wildcard initializer row missing ({init}..{}): {dead:?}",
        init + 1
    );
}

/// Feature-off posture: `_` is an ordinary name — the `late var _ = e`
/// initializer is live code.
#[test]
fn ticket_3182_late_wildcard_silent_without_feature() {
    let report = report_ops_wildcards("void f() {\n  late var _ = 0;\n}", false);
    assert!(
        !code_names(&report).iter().any(|name| *name == "dead_code"),
        "{:?}",
        rows(&report)
    );
}

/// A non-`late` `_` initializer and a `late` non-`_` name both stay
/// live — the face needs both the keyword and the wildcard spelling.
#[test]
fn ticket_3182_late_named_and_nonlate_wildcard_stay_live() {
    let report = report_ops("void f() {\n  var _ = 0;\n  late var x = 0;\n}");
    assert!(
        !code_names(&report).iter().any(|name| *name == "dead_code"),
        "{:?}",
        rows(&report)
    );
}
