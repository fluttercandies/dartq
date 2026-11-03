//! The #3837 `dead_code` write-target pins — the null-aware dead chain
//! (`_verifyUnassignedSimpleIdentifier`, upstream Dart SDK source
//! pkg/analyzer/lib/src/error/dead_code_verifier.dart:467-505) retires
//! when the access IS the write's own target: the verifier faces are
//! read-only call sites (`verifyIndexExpression` fires once, from the
//! index READ resolution — resolver.dart:3404), so `l?[0] = v` was always
//! silent, and the `++`/`--` compound writes — `l?[0]++`, `++l?[0]`,
//! `c?.x++`, `c?.x--` — are silent for the same reason.
//!
//! Two shapes keep their rows:
//!
//! * A chain that already climbed past its root: `l?[0][1]++` writes the
//!   OUTER `[1]` while the inner `?[` read still reports `?[0][1]`
//!   (`while (parent is MethodInvocation || parent is PropertyAccess ||
//!   parent is IndexExpression)`, dead_code_verifier.dart:492-497).
//! * A cascade chain: the `++` attaches OUTSIDE the cascade section, so
//!   `c?..x++`'s first section still answers
//!   `verifyCascadeExpression`'s gate (`?..x` reports).

use crate::diagnose::test_rig::{CORE_PRELUDE, code_names, report_ops, rows};

/// `(offset, length)` pairs of the `dead_code` rows, rebased to the
/// fixture's own text (the prelude's length subtracts out).
fn dead_rows(source: &str) -> Vec<(usize, usize)> {
    let report = report_ops(source);
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "dead_code")
        .map(|d| {
            let offset = d.range.offset as usize - CORE_PRELUDE.len();
            (offset, offset + d.range.length as usize)
        })
        .collect()
}

fn has_dead_code(source: &str) -> bool {
    code_names(&report_ops(source))
        .iter()
        .any(|name| *name == "dead_code")
}

/// `l?[0]++` — the `?[` access resolves for write; upstream reports
/// nothing (oracle 3.13.7: no `dead_code`).
#[test]
fn ticket_3837_postfix_index_increment_silent() {
    let source = "void f() {\n  List<int>? l;\n  l?[0]++;\n}";
    assert!(
        !has_dead_code(source),
        "`l?[0]++` must stay silent: {:?}",
        rows(&report_ops(source))
    );
}

/// `++l?[0]` — the prefix twin: the same write resolution, the same
/// silence.
#[test]
fn ticket_3837_prefix_index_increment_silent() {
    let source = "void f() {\n  List<int>? l;\n  ++l?[0];\n}";
    assert!(
        !has_dead_code(source),
        "`++l?[0]` must stay silent: {:?}",
        rows(&report_ops(source))
    );
}

/// `l?[0]--` / `--l?[0]` — the decrement pair.
#[test]
fn ticket_3837_index_decrement_silent() {
    for op in ["l?[0]--;", "--l?[0];"] {
        let source = format!("void f() {{\n  List<int>? l;\n  {op}\n}}");
        assert!(
            !has_dead_code(&source),
            "`{op}` must stay silent: {:?}",
            rows(&report_ops(&source))
        );
    }
}

/// `c?.x++` — the member-access twin retires through the same gate.
#[test]
fn ticket_3837_postfix_member_increment_silent() {
    let source = "class C { int x = 0; }\nvoid f() {\n  C? c;\n  c?.x++;\n}";
    assert!(
        !has_dead_code(source),
        "`c?.x++` must stay silent: {:?}",
        rows(&report_ops(source))
    );
}

/// `++c?.x` / `c?.x--` / `--c?.x` — prefix and decrement forms.
#[test]
fn ticket_3837_member_write_forms_silent() {
    for stmt in ["++c?.x;", "c?.x--;", "--c?.x;"] {
        let source = format!("class C {{ int x = 0; }}\nvoid f() {{\n  C? c;\n  {stmt}\n}}");
        assert!(
            !has_dead_code(&source),
            "`{stmt}` must stay silent: {:?}",
            rows(&report_ops(&source))
        );
    }
}

/// `l?[0]` — the read form still reports `?[0]` (regression pin: the
/// retire only covers write targets).
#[test]
fn ticket_3837_index_read_reports() {
    let source = "void f() {\n  List<int>? l;\n  l?[0];\n}";
    let dead = dead_rows(source);
    let head = source.find("?[").unwrap();
    assert_eq!(
        dead,
        vec![(head, head + 4)],
        "`l?[0]` must report `?[0]`: {dead:?}"
    );
}

/// `l?[0] = 1` — the plain assignment form stays silent (the
/// `handle_assignment_expression` retire already owned it).
#[test]
fn ticket_3837_index_assign_silent() {
    let source = "void f() {\n  List<int>? l;\n  l?[0] = 1;\n}";
    assert!(
        !has_dead_code(source),
        "`l?[0] = 1` must stay silent: {:?}",
        rows(&report_ops(source))
    );
}

/// `c?.x = 1` — the member-write twin.
#[test]
fn ticket_3837_member_assign_silent() {
    let source = "class C { int x = 0; }\nvoid f() {\n  C? c;\n  c?.x = 1;\n}";
    assert!(
        !has_dead_code(source),
        "`c?.x = 1` must stay silent: {:?}",
        rows(&report_ops(source))
    );
}

/// `l?[0][1]++` — the `++` writes the OUTER `[1]`; the inner `?[` read's
/// climbed row `?[0][1]` still reports (oracle 3.13.7: `?[0][1]`, 7
/// columns).
#[test]
fn ticket_3837_nested_index_outer_write_keeps_chain() {
    let source = "void f() {\n  List<List<int>>? l;\n  l?[0][1]++;\n}";
    let dead = dead_rows(source);
    let head = source.find("?[").unwrap();
    assert_eq!(
        dead,
        vec![(head, head + 7)],
        "`l?[0][1]++` must keep `?[0][1]`: {dead:?}"
    );
}

/// `c?.x.y++` — the `++` writes `.y`; the `?.x` chain already climbed to
/// `.y` and still reports `?.x.y` (oracle 3.13.7: `?.x.y`, 5 columns).
/// Ignored: the merge layer's chain `end` currently stays at the `?.`
/// operator for the plain-`.` continuation `c?.x.y` (a `(op, op)` tuple
/// reaches teardown and the row saturates to zero length). The grow lives
/// in `merge_member`'s containing-chain arm (invocation.rs), outside this
/// ticket's scope — see the lane report for the dependency.
#[test]
#[ignore = "chain end pinned at op for `c?.x.y` — invocation.rs grow path"]
fn ticket_3837_climbed_member_write_keeps_chain() {
    let source = "class C { C x = C(); C y = C(); }\nvoid f() {\n  C? c;\n  c?.x.y++;\n}";
    let dead = dead_rows(source);
    let head = source.find("?.x.y").unwrap();
    assert_eq!(
        dead,
        vec![(head, head + 5)],
        "`c?.x.y++` must keep `?.x.y`: {dead:?}"
    );
}

/// `c?..x++` — the cascade's first section still answers the verifier's
/// gate, so `?..x` reports even under the trailing `++` (oracle 3.13.7:
/// `?..x`, 4 columns).
#[test]
fn ticket_3837_cascade_member_keeps_row_on_increment() {
    let source = "class C { int x = 0; }\nvoid f() {\n  C? c;\n  c?..x++;\n}";
    let dead = dead_rows(source);
    let head = source.find("?..x").unwrap();
    assert_eq!(
        dead,
        vec![(head, head + 4)],
        "`c?..x++` must keep `?..x`: {dead:?}"
    );
}
