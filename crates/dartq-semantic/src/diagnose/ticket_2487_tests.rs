//! The #2487 pins: the assignment-target scan's bounded form and the
//! is-check undefined head's id-keyed set.
//!
//! F5 (`assignability.rs`): `assignment_target_index` replaces the
//! reverse-linear `.rev().find` `mark_assignment`/`check_assignment`/
//! `check_compound_assignment` each ran over the whole row table. The
//! rows are offset-sorted, so a `partition_point` bounds the scan to
//! the pre-operator prefix (`end() <= limit` implies `offset <= limit`
//! — no verdict-eligible row sits past the bound); `end()` is not
//! monotone inside the prefix, so the verdict still reads a bounded
//! reverse scan there. The pins prove the verdict is unchanged (simple,
//! compound, and index targets) and that the scan's predicate visits
//! stay confined to the prefix when the right-hand side carries
//! thousands of rows (`ASSIGNMENT_TARGET_VISITS` — the instrumented
//! `cfg(test)` twin counts them).
//!
//! F6 (`resolved_ops.rs`): `tested_arg_undefined` consults the
//! `undefined_ids` set `observe_names` extends beside the `name_ty`
//! cache — the same interned `StringId` the scan already computes —
//! instead of re-interning every probe's spelling (the `id_overloaded`
//! pattern). The verdict's semantics are the #2450 family's
//! (`x is Missing` reports `type_test_with_undefined_name` and keeps
//! its flow face); these pins prove the id-keyed set answers the same
//! for the undefined, registered, kind-spelling, and scoped-parameter
//! faces through the mounted `ResolvedTypeOps`.

use super::super::types::FileReport;
use super::async_return_tests::{code_names, report};
use super::{ASSIGNMENT_TARGET_MAX_TAIL, ASSIGNMENT_TARGET_VISITS};
use crate::diagnose::test_rig::report_ops;

fn rows(report: &FileReport, code: &str) -> Vec<(u32, u32)> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| (d.range.offset, d.range.length))
        .collect()
}

// —— F5: the bounded assignment-target scan ——————————————————————————

/// The plain `=` face over a wide right-hand side: `a = <4000-term
/// int chain>` still lands the assignability verdict the single scan
/// produced — `invalid_assignment` at the whole right-hand expression
/// (the ticket_3145 `y = B().x` verdict over a multi-row window).
#[test]
fn assignment_wide_rhs_reports() {
    let tail = "b.x + ".repeat(4000);
    let source = format!(
        "class B {{ int x = 2; }}\n\
         void main() {{ B b = B(); String a; a = {tail}b.x; }}"
    );
    let report = report(&source);
    assert_eq!(
        rows(&report, "invalid_assignment").len(),
        1,
        "{:?}",
        report.diagnostics
    );
}

/// The compound face reads the same helper: `i += <wide String chain>`
/// keeps its verdict — the `int.+` parameter refuses `String` (ATNA)
/// and the `num` result refuses the `int` write-back
/// (`invalid_assignment`; the ticket_2967 fixture family's shape,
/// assignment_expression_resolver.dart:308-313/368-372).
#[test]
fn compound_assignment_wide_rhs_reports() {
    let tail = "\"x\" + ".repeat(4000);
    let source = format!(
        "class int {{}}\nclass num {{}}\nclass String {{}}\n\
         void main() {{ int i = 1; i += {tail}\"x\"; }}"
    );
    let report = report(&source);
    assert!(
        code_names(&report)
            .iter()
            .any(|code| *code == "invalid_assignment"),
        "{:?}",
        report.diagnostics
    );
}

/// The index-write face over a wide right-hand side: `l[c] = <4000-term
/// int chain>` skips the folded index-result value row to find the `l`
/// target — the index argument `c` keeps its read credit and the whole
/// file stays silent (the ticket_2423 `folded_index_target` verdict
/// over a multi-row tail, the ticket_2436 `operator []=` decl).
#[test]
fn index_assignment_skips_the_folded_value_row() {
    let tail = "1 + ".repeat(4000);
    let source = format!(
        "class int {{}}\n\
         class L {{ void operator []=(int i, int v) {{}} }}\n\
         int go(L l, int m) {{\n\
                    \x20 var c = m;\n\
                    \x20 l[c] = {tail}1;\n\
                    \x20 return c;\n\
                    \x20 }}"
    );
    let report = report(&source);
    assert!(code_names(&report).is_empty(), "{:?}", code_names(&report));
}

/// The bounded scan's instruction count: the last assignment's
/// right-hand side is a 4000-part interpolation — its element rows stay
/// live in the row table at check time (`interpolation_child_rows`
/// exists precisely because they persist), so the old `.rev().find`
/// walked all ~4000 post-operator rows while the bounded scan visits
/// only the pre-operator prefix.
#[test]
fn assignment_target_visits_stay_in_the_prefix() {
    let tail = "b.x + ".repeat(4000);
    let chain = "\"x\" + ".repeat(4000);
    let interp = "${b.x}".repeat(4000);
    let source = format!(
        "class B {{ int x = 2; }}\nclass num {{}}\n\
         void main() {{ B b = B(); String a; a = {tail}b.x; \
         int i = 1; i += {chain}\"x\"; int c; c = \"{interp}\"; }}"
    );
    ASSIGNMENT_TARGET_VISITS.with(|visits| visits.set(0));
    ASSIGNMENT_TARGET_MAX_TAIL.with(|tail| tail.set(0));
    let report = report(&source);
    let visits = ASSIGNMENT_TARGET_VISITS.with(|visits| visits.get());
    let max_tail = ASSIGNMENT_TARGET_MAX_TAIL.with(|tail| tail.get());
    // The interpolation assignment's element rows stay live at check
    // time: at least one call saw a >4000-row post-operator tail the old
    // `.rev().find` walked element by element.
    assert!(
        max_tail > 4000,
        "the fixture must produce a wide post-operator tail somewhere \
         (max tail {max_tail}): {:?}",
        report.diagnostics
    );
    assert!(
        visits < 256,
        "the bounded scan visited {visits} rows across every call — the \
         prefix must stay small no matter how wide the tail (max tail \
         {max_tail}): {:?}",
        report.diagnostics
    );
    // The verdicts still land through the bounded scan: two `=` faces
    // (`int` chains into `String`) plus the compound's `num` result into
    // the `int` target.
    assert_eq!(
        rows(&report, "invalid_assignment").len(),
        3,
        "{:?}",
        report.diagnostics
    );
}

// —— F6: the id-keyed undefined-tested-head set ——————————————————————

/// The undefined face through the set: `x is Missing` reports
/// `type_test_with_undefined_name` on the spelling — the same verdict
/// the interning read produced (#2450's family).
#[test]
fn undefined_tested_head_reports_its_code() {
    let source = "void f(int x) {\n\
                      \x20 if (x is Missing) {\n\
                      \x20   x;\n\
                      \x20 }\n\
                      }";
    let report = report_ops(source);
    assert!(
        code_names(&report)
            .iter()
            .any(|name| *name == "type_test_with_undefined_name"),
        "{:?}",
        report.diagnostics
    );
}

/// A registered container's spelling never lands in the set: `x is
/// Defined` reports no undefined-name code — the scan's
/// `registry.contains_key` arm keeps it out.
#[test]
fn defined_tested_head_stays_out_of_the_set() {
    let source = "class Defined {}\n\
                      void f(Object x) {\n\
                      \x20 if (x is Defined) {\n\
                      \x20   x;\n\
                      \x20 }\n\
                      }";
    let report = report_ops(source);
    assert!(
        !code_names(&report)
            .iter()
            .any(|name| *name == "type_test_with_undefined_name"),
        "{:?}",
        report.diagnostics
    );
}

/// The kind spellings never land in the set either: `x is Null` and
/// `x is Never` resolve through the lexeme arm — no undefined-name
/// code on either probe.
#[test]
fn kind_spellings_stay_out_of_the_set() {
    let source = "void f(int x) {\n\
                      \x20 if (x is Null) {\n\
                      \x20   x;\n\
                      \x20 }\n\
                      \x20 if (x is Never) {\n\
                      \x20   x;\n\
                      \x20 }\n\
                      }";
    let report = report_ops(source);
    assert!(
        !code_names(&report)
            .iter()
            .any(|name| *name == "type_test_with_undefined_name"),
        "{:?}",
        report.diagnostics
    );
}

/// A scoped type parameter resolves through `scoped_param`: `x is T`
/// inside `f<T>` reports no undefined-name code — the set's
/// `scoped_param` exclusion keeps the parameter spelling out.
#[test]
fn scoped_parameter_tested_head_stays_out_of_the_set() {
    let source = "void f<T>(T x) {\n\
                      \x20 if (x is T) {\n\
                      \x20   x;\n\
                      \x20 }\n\
                      }";
    let report = report_ops(source);
    assert!(
        !code_names(&report)
            .iter()
            .any(|name| *name == "type_test_with_undefined_name"),
        "{:?}",
        report.diagnostics
    );
}

/// The undefined head still refuses the promotion step through the
/// set: the then branch reads `x` under its declared `int` face, so
/// the member read reports `undefined_getter` — the verdict the
/// interning gate produced.
#[test]
fn undefined_tested_head_promotes_nothing() {
    let source = "void f(int x) {\n\
                      \x20 if (x is Missing) {\n\
                      \x20   x.foot;\n\
                      \x20 }\n\
                      }";
    let report = report_ops(source);
    assert!(
        rows(&report, "dead_code").is_empty(),
        "the then branch stays live: {:?}",
        report.diagnostics
    );
    assert!(
        code_names(&report)
            .iter()
            .any(|name| *name == "undefined_getter"),
        "the member read resolves against the declared `int` face: {:?}",
        report.diagnostics
    );
}
