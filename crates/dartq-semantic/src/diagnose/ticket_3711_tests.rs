//! #3711 — dot-shorthand M2: a shorthand that roots a selector chain
//! (`.member().field`, `.member().method()`, `.new(1).field`, and the bare
//! `.member()` send) has its member-name row consumed by the invocation /
//! member merges before the shorthand sweep can verdict it. The sweep's
//! `is_shorthand_head` requires `qualified && Bare(_)` with `ty == UNKNOWN`;
//! the chain's `.field` merge rewrites the row into `Member{recv}` and the
//! send close can write a non-UNKNOWN `ty`, so the row starves — silence
//! where the oracle emits `dot_shorthand_undefined_member`.
//!
//! Oracle provenance: `conformance/expected.tsv` —
//! `equality_selector_chain_error_test` reports
//! `The static method or constructor 'member' isn't defined for the
//! context type '_'.` at every `.member` / `.new` selector that roots a
//! chain under `==`/`!=`, both sides, and inside `if` conditions; the
//! `rhsNeedsToBeShorthand` nested shapes answer `'_'` identically; the
//! `objectContextType` shapes answer `'Object'`; the resolved `.new(1)`
//! chain reports `extra_positional_arguments` at `(` plus
//! `undefined_getter`/`undefined_method` at the continuation selector and
//! NO `dot_shorthand_undefined_member` (the constructor resolved).

use super::test_rig::{CORE_PRELUDE, assert_silent, report_ops, rows};
use super::types::FileReport;

/// `fixture.find` past the mounted `CORE_PRELUDE` — `needle` names the
/// shorthand with its leading `.`, but the diagnostic anchors at the
/// selector token (`memberName` upstream), one past the dot.
fn at(fixture: &str, needle: &str) -> usize {
    CORE_PRELUDE.len() + fixture.find(needle).expect(needle) + 1
}

/// The `dot_shorthand_undefined_member` rows as `code @offset: message`.
fn shorthand_rows(report: &FileReport) -> Vec<String> {
    rows(report)
        .into_iter()
        .filter(|row| row.starts_with("dot_shorthand_undefined_member"))
        .collect()
}

// ---------------------------------------------------------------------------
// Chain shapes — `.member().selector` consumes the shorthand's row at the
// second dot's merge.
// ---------------------------------------------------------------------------

/// `.member().field == m` — the head resolves on `SM`, so the `.field`
/// merge claims the `.member` row as its receiver; the oracle still
/// verdicts the shorthand against the unknown context (`'_'`).
#[test]
fn chain_member_field_reports_underscore() {
    let fixture = "class SM { static SM member() => SM(); int field = 0; }\n\
                   void f(SM m) { bool b = .member().field == m; }\n";
    let report = report_ops(fixture);
    assert_eq!(
        shorthand_rows(&report),
        [format!(
            "dot_shorthand_undefined_member @{}: The static method or constructor 'member' isn't defined for the context type '_'.",
            at(fixture, ".member"),
        )],
        "rows: {:?}",
        rows(&report),
    );
}

/// `.member().method() == m` — the invocation arm's same consumption
/// through a chained send.
#[test]
fn chain_member_method_reports_underscore() {
    let fixture = "class SM { static SM member() => SM(); SM method() => SM(); }\n\
                   void f(SM m) { bool b = .member().method() == m; }\n";
    let report = report_ops(fixture);
    assert_eq!(
        shorthand_rows(&report),
        [format!(
            "dot_shorthand_undefined_member @{}: The static method or constructor 'member' isn't defined for the context type '_'.",
            at(fixture, ".member"),
        )],
        "rows: {:?}",
        rows(&report),
    );
}

/// `.member() == arg` — the bare invoked shorthand's row is likewise
/// consumed before the sweep; the oracle reports `'_'`.
#[test]
fn bare_member_invoke_reports_underscore() {
    let fixture = "class SM { static SM member() => SM(); }\n\
                   void f(SM arg) { bool b = .member() == arg; }\n";
    let report = report_ops(fixture);
    assert_eq!(
        shorthand_rows(&report),
        [format!(
            "dot_shorthand_undefined_member @{}: The static method or constructor 'member' isn't defined for the context type '_'.",
            at(fixture, ".member"),
        )],
        "rows: {:?}",
        rows(&report),
    );
}

/// `.new() == x` — the ctor selector's row rides the same fold; the
/// oracle reports `'_'`.
#[test]
fn bare_new_invoke_reports_underscore() {
    let fixture = "class SM {}\n\
                   void f(SM x) { bool b = .new() == x; }\n";
    let report = report_ops(fixture);
    assert_eq!(
        shorthand_rows(&report),
        [format!(
            "dot_shorthand_undefined_member @{}: The static method or constructor 'new' isn't defined for the context type '_'.",
            at(fixture, ".new"),
        )],
        "rows: {:?}",
        rows(&report),
    );
}

/// `.new(1).field == c` — the chained ctor selector's same verdict.
#[test]
fn chain_new_field_reports_underscore() {
    let fixture = "class CWNF { CWNF(this.x); int x = 0; CWNF get field => CWNF(0); }\n\
                   void f(CWNF c) { bool b = .new(1).field == c; }\n";
    let report = report_ops(fixture);
    assert_eq!(
        shorthand_rows(&report),
        [format!(
            "dot_shorthand_undefined_member @{}: The static method or constructor 'new' isn't defined for the context type '_'.",
            at(fixture, ".new"),
        )],
        "rows: {:?}",
        rows(&report),
    );
}

// ---------------------------------------------------------------------------
// Negative controls — the fix must not fabricate rows.
// ---------------------------------------------------------------------------

/// `obj.member().field == m` — a REAL receiver's selector chain is no
/// shorthand; every member resolves and the oracle stays silent.
#[test]
fn non_shorthand_chain_stays_silent() {
    let fixture = "class SM { static SM member() => SM(); int field = 0; SM obj = SM(); }\n\
                   void f(SM m) { SM obj = SM(); bool b = obj.member().field == m; }\n";
    let report = report_ops(fixture);
    assert!(
        shorthand_rows(&report).is_empty(),
        "a real receiver's chain never reports the shorthand face: {:?}",
        rows(&report),
    );
}

/// `C c = .member()` — a shorthand whose member DOES resolve against the
/// armed declaration context stays silent (the sweep verdicts a hit).
#[test]
fn resolved_shorthand_stays_silent() {
    let fixture = "class C { static C member() => C(); }\n\
                   void f() { C c = .member(); c.member(); }\n";
    assert_silent(&report_ops(fixture));
}

/// `if (.member() == arg)` — the same shorthand in a condition position;
/// the oracle reports `'_'` here too.
#[test]
fn condition_member_invoke_reports_underscore() {
    let fixture = "class SM { static SM member() => SM(); }\n\
                   void f(SM arg) { if (.member() == arg) {} }\n";
    let report = report_ops(fixture);
    assert_eq!(
        shorthand_rows(&report),
        [format!(
            "dot_shorthand_undefined_member @{}: The static method or constructor 'member' isn't defined for the context type '_'.",
            at(fixture, ".member"),
        )],
        "rows: {:?}",
        rows(&report),
    );
}

/// `b = .other() == arg` — the unresolved-head twin: the invocation fold
/// cannot claim `other` anywhere, so the row survives for the sweep
/// (already green pre-fix — regression pin).
#[test]
fn unresolved_member_invoke_reports_underscore() {
    let fixture = "class SM {}\nvoid f(SM arg) { bool b = .other() == arg; }\n";
    let report = report_ops(fixture);
    assert_eq!(
        shorthand_rows(&report),
        [format!(
            "dot_shorthand_undefined_member @{}: The static method or constructor 'other' isn't defined for the context type '_'.",
            at(fixture, ".other"),
        )],
        "rows: {:?}",
        rows(&report),
    );
}
